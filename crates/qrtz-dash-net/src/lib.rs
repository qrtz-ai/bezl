//! ADR 0231 amendment 1 (Bozniak): Agent Dash's only network code, in its own process.
//!
//! - Egress is pinned: every request goes to [`ORIGIN`] (`https://dash.qrtz.ai`) and nowhere else. The origin is a
//!   constant, the path comes from a fixed table ([`plan`]), every id is a checked path segment, the full URL is
//!   checked again ([`egress_ok`]) right before it is sent, and redirects are off, so plain http, another host, a
//!   port, userinfo or a redirect are all refused.
//! - The app talks to it only over stdio, one JSON line per `dash:*` call ([`handle_line`]); each line names the
//!   channel's contract capability (`dash.read` or `dash.manage`) and is refused if it doesn't match
//!   contracts/ipc-contract.json.
//! - It sees only `dash:*` parameters (ids, keys, scope, the one-time setup token) and the server's already
//!   scrubbed dash view. It never gets the vault, note text, transcripts or the app's keys. The dashboard's owner
//!   key is this process's own secret: it is created by `dash:connect`, kept in a Keychain item only this helper
//!   reads ([`KEYCHAIN_SERVICE`]), and is never sent to the app.
use qrtz_ipc::{AppError, ErrorCode};
use serde_json::{json, Value};
use zeroize::{Zeroize, Zeroizing};

/// The one origin this process may reach.
pub const ORIGIN: &str = "https://dash.qrtz.ai";
pub const KEYCHAIN_SERVICE: &str = "ai.qrtz.dash-net";
pub const KEYCHAIN_ACCOUNT: &str = "owner-key";
/// Longest stdin line accepted (a `dash:*` call is a few hundred bytes).
pub const MAX_LINE: usize = 16 * 1024;
/// Largest server response body read (Bozniak M2).
pub const MAX_RESPONSE: usize = 1024 * 1024;
/// Compiled into the helper binary (Bozniak M5): scripts/release/verify-bundle.sh greps every bundle file for it, so
/// a renamed copy of the helper still fails the release gate. Keep the two in sync (a test checks).
pub const BUNDLE_MARKER: &str = "qrtz-dash-net:adr-0231:never-in-a-release-bundle:5c1e9b";

/// Zeroes a `setupToken` string in place (Bozniak L).
pub fn scrub_setup_token(p: &mut Value) {
    if let Some(Value::String(t)) = p.get_mut("setupToken") {
        t.zeroize();
    }
}

/// The answer to a line over [`MAX_LINE`] (its id is never parsed).
pub fn line_too_long() -> Value {
    json!({"id": Value::Null, "error": bad("line too long").to_json()})
}

/// One line read with a byte cap (Bozniak M1).
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// The line, without its newline.
    Ok(Vec<u8>),
    /// Longer than the cap: dropped, and the input drained to the next newline.
    TooLong,
    Eof,
}

/// Reads one line holding at most `max` bytes in memory: `read_until` capped at `max + 1`, then, if the line is
/// longer, drains (and discards) up to the next newline in 64 KB pieces.
pub async fn read_line_capped<R: tokio::io::AsyncBufRead + Unpin>(
    r: &mut R,
    max: usize,
) -> std::io::Result<Line> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut buf = Vec::new();
    let n = (&mut *r).take(max as u64 + 1).read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(Line::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        return Ok(Line::Ok(buf));
    }
    if buf.len() <= max {
        return Ok(Line::Ok(buf)); // the last line, with no newline
    }
    loop {
        let mut sink = Vec::new();
        let k = (&mut *r).take(64 * 1024).read_until(b'\n', &mut sink).await?;
        if k == 0 || sink.last() == Some(&b'\n') {
            return Ok(Line::TooLong);
        }
    }
}

/// The channels this helper answers, with the capability each needs (must match ipc-contract.json).
pub const CHANNELS: &[(&str, &str)] = &[
    ("dash:view", "dash.read"),
    ("dash:connect", "dash.manage"),
    ("dash:disconnect", "dash.manage"),
    ("dash:issue-code", "dash.manage"),
    ("dash:rotate-codes", "dash.manage"),
    ("dash:review-agent", "dash.manage"),
    ("dash:delete", "dash.manage"),
];

fn bad(m: &str) -> AppError {
    AppError::new(ErrorCode::Validation, m).retryable(false)
}

/// `true` only for `https://dash.qrtz.ai/<path>`: no other scheme, host, port, userinfo, fragment or `..`.
pub fn egress_ok(url: &str) -> bool {
    let Some(path) = url.strip_prefix(ORIGIN) else {
        return false;
    };
    path.starts_with('/')
        && !path.contains("..")
        && !path.contains('#')
        && !path.contains('@')
        && !path.contains('\\')
        && path.bytes().all(|b| b.is_ascii_graphic())
}

/// An id or widget key that can go in a path segment as is.
pub fn segment(s: &str) -> Result<&str, AppError> {
    if !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        Ok(s)
    } else {
        Err(bad("bad id or key"))
    }
}

/// `ok_<32 hex>_<64 hex>` (services/dash-worker OWNER_KEY).
pub fn is_owner_key(k: &str) -> bool {
    let b = k.as_bytes();
    b.len() == 3 + 32 + 1 + 64
        && k.starts_with("ok_")
        && b[35] == b'_'
        && b[3..35]
            .iter()
            .chain(&b[36..])
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

/// One server request: method, path under [`ORIGIN`], optional JSON body.
#[derive(Debug, PartialEq)]
pub struct Plan {
    pub method: Method,
    pub path: String,
    pub body: Option<Value>,
}

/// The owner route for a `dash:*` channel (not `dash:connect`/`dash:disconnect`, which [`Helper`] handles).
pub fn plan(channel: &str, p: &Value) -> Result<Plan, AppError> {
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let mk = |method, path: String, body| Ok(Plan { method, path, body });
    match channel {
        "dash:view" => mk(Method::Get, "/v1/owner/view".into(), None),
        "dash:issue-code" => mk(Method::Post, "/v1/owner/codes".into(), None),
        "dash:rotate-codes" => mk(Method::Post, "/v1/owner/codes/rotate".into(), None),
        "dash:review-agent" => {
            let id = s("agentId");
            let approve = p.get("approve").and_then(Value::as_bool).unwrap_or(false);
            mk(
                Method::Post,
                format!("/v1/owner/agents/{}/review", segment(&id)?),
                Some(json!({ "approve": approve })),
            )
        }
        "dash:delete" => {
            let path = match p.get("scope").and_then(Value::as_str) {
                Some("widget") => {
                    let (id, key) = (s("agentId"), s("key"));
                    format!("/v1/owner/widgets/{}/{}", segment(&id)?, segment(&key)?)
                }
                Some("agent") => format!("/v1/owner/agents/{}", segment(&s("agentId"))?),
                Some("dashboard") => "/v1/owner/dashboard".to_owned(),
                _ => return Err(bad("scope is widget, agent or dashboard")),
            };
            mk(Method::Delete, path, None)
        }
        _ => Err(qrtz_ipc::no_handler(channel)),
    }
}

/// Where the owner key lives: the macOS Keychain in the helper; memory in tests; nowhere elsewhere.
pub trait KeyStore: Send + Sync {
    fn load(&self) -> Option<String>;
    fn save(&self, key: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

/// Bozniak M6: the release keychain group. Only the helper's entitlements
/// (entitlements/ai.qrtz.dash-net.entitlements) hold it; the app doesn't, so it can't read the item.
pub const ACCESS_GROUP: &str = "MR26CCZ496.ai.qrtz.dash-net";

/// The owner key in the macOS Keychain.
///
/// Release builds (signed, with the entitlements above): the data-protection keychain, access group
/// [`ACCESS_GROUP`], `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` (no iCloud sync, no backup restore to another Mac).
/// Debug builds: the login keychain (an unsigned dev binary can't hold a keychain group); dash only runs in debug
/// builds today. The release path needs a signed Mac build to verify (ADR 0231).
#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
fn keychain_options(write: bool) -> Result<security_framework::passwords::PasswordOptions, String> {
    use security_framework::passwords::PasswordOptions;
    let mut o = PasswordOptions::new_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT);
    if cfg!(not(debug_assertions)) {
        o.use_protected_keychain();
        o.set_access_group(ACCESS_GROUP);
        if write {
            use security_framework::access_control::{ProtectionMode, SecAccessControl};
            let ac = SecAccessControl::create_with_protection(
                Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
                0,
            )
            .map_err(|e| e.to_string())?;
            o.set_access_control(ac);
        }
    }
    Ok(o)
}

#[cfg(target_os = "macos")]
impl KeyStore for KeychainStore {
    fn load(&self) -> Option<String> {
        security_framework::passwords::generic_password(keychain_options(false).ok()?)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .filter(|k| is_owner_key(k))
    }
    fn save(&self, key: &str) -> Result<(), String> {
        // Delete first, so an older item can't keep weaker attributes through an update.
        self.delete()?;
        security_framework::passwords::set_generic_password_options(key.as_bytes(), keychain_options(true)?)
            .map_err(|e| e.to_string())
    }
    fn delete(&self) -> Result<(), String> {
        const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
        match security_framework::passwords::delete_generic_password_options(keychain_options(false)?) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// No Keychain: the server can't be used on this platform (the key would have to go to disk).
pub struct NoStore;

impl KeyStore for NoStore {
    fn load(&self) -> Option<String> {
        None
    }
    fn save(&self, _: &str) -> Result<(), String> {
        Err("the dash server needs the macos keychain".into())
    }
    fn delete(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Tests only.
#[derive(Default)]
pub struct MemoryStore(pub std::sync::Mutex<Option<String>>);

impl KeyStore for MemoryStore {
    fn load(&self) -> Option<String> {
        self.0.lock().ok().and_then(|k| k.clone())
    }
    fn save(&self, key: &str) -> Result<(), String> {
        *self.0.lock().map_err(|e| e.to_string())? = Some(key.to_owned());
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        *self.0.lock().map_err(|e| e.to_string())? = None;
        Ok(())
    }
}

pub fn platform_store() -> Box<dyn KeyStore> {
    #[cfg(target_os = "macos")]
    {
        Box::new(KeychainStore)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoStore)
    }
}

/// Sends one request. The only place this crate touches the network.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// `url` has passed [`egress_ok`]; `bearer` goes in the Authorization header only.
    async fn send(
        &self,
        m: Method,
        url: &str,
        bearer: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), AppError>;
}

fn unavailable() -> AppError {
    AppError::new(ErrorCode::Unavailable, "the dash server can't be reached").retryable(true)
}

/// The real transport: reqwest with rustls, a 10 s timeout, no redirects, https only and no proxy.
pub struct Https(reqwest::Client);

impl Https {
    /// Bozniak M3: no fallback. If this client can't be built there is no client (a default one would follow
    /// redirects, allow plain http and use proxies), and the helper exits.
    pub fn new() -> Result<Self, AppError> {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .no_proxy()
            .build()
            .map(Https)
            .map_err(|_| unavailable())
    }
}

#[async_trait::async_trait]
impl Transport for Https {
    async fn send(
        &self,
        m: Method,
        url: &str,
        bearer: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), AppError> {
        let method = match m {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut rb = self.0.request(method, url).bearer_auth(bearer);
        if let Some(b) = body {
            rb = rb
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&b).unwrap_or_default());
        }
        // Errors are mapped to one fixed message: nothing from the URL or headers reaches the app.
        let r = rb.send().await.map_err(|_| unavailable())?;
        let status = r.status().as_u16();
        // Bozniak M2: read in chunks and stop past MAX_RESPONSE (never r.bytes(), which buffers it all).
        let mut r = r;
        let mut bytes = Vec::new();
        while let Some(c) = r.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + c.len() > MAX_RESPONSE {
                return Err(
                    AppError::new(ErrorCode::Unavailable, "the dash server sent too much").retryable(false),
                );
            }
            bytes.extend_from_slice(&c);
        }
        Ok((status, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
    }
}

/// The helper's state: one transport, one key store.
pub struct Helper {
    pub transport: Box<dyn Transport>,
    pub store: Box<dyn KeyStore>,
}

fn status_error(status: u16, body: &Value) -> AppError {
    let msg = body["error"]
        .as_str()
        .unwrap_or("the dash server refused the request")
        .chars()
        .take(200)
        .collect::<String>();
    match status {
        401 | 403 => AppError::new(ErrorCode::Forbidden, msg).retryable(false),
        404 | 410 => AppError::new(ErrorCode::NotFound, msg).retryable(false),
        400..=499 => AppError::new(ErrorCode::Validation, msg).retryable(false),
        _ => AppError::new(ErrorCode::Unavailable, msg).retryable(true),
    }
}

impl Helper {
    async fn send(
        &self,
        m: Method,
        path: &str,
        bearer: &str,
        body: Option<Value>,
    ) -> Result<Value, AppError> {
        let url = format!("{ORIGIN}{path}");
        // Checked again right before the request leaves (Bozniak: refuse other hosts and plain http).
        if !egress_ok(&url) {
            return Err(bad("the dash helper only talks to https://dash.qrtz.ai"));
        }
        let (status, mut body) = self.transport.send(m, &url, bearer, body).await?;
        if !(200..300).contains(&status) {
            return Err(status_error(status, &body));
        }
        if let Some(o) = body.as_object_mut() {
            o.remove("readme_version");
            // The owner key never goes back to the app.
            o.remove("owner_key");
        }
        Ok(body)
    }

    /// One `dash:*` call.
    pub async fn call(&self, channel: &str, p: &Value) -> Result<Value, AppError> {
        match channel {
            "dash:connect" => {
                if let Some(token) = p.get("setupToken").and_then(Value::as_str) {
                    // Bozniak L: the setup token is held zeroized for as long as it is held.
                    let token = Zeroizing::new(token.trim().to_owned());
                    if token.is_empty() || token.len() > 256 {
                        return Err(bad("setup token missing"));
                    }
                    return self.connect_raw(&token).await;
                }
                // No token: reuse the dashboard this helper already holds a key for.
                if self.store.load().is_some() {
                    Ok(json!({"connected": true, "origin": ORIGIN}))
                } else {
                    Err(AppError::new(ErrorCode::NotFound, "no dash server connected yet").retryable(false))
                }
            }
            "dash:disconnect" => {
                self.store.delete().map_err(|e| {
                    AppError::new(ErrorCode::Internal, format!("keychain: {e}")).retryable(false)
                })?;
                Ok(json!({"ok": true}))
            }
            _ => {
                let plan = plan(channel, p)?;
                let key = self.store.load().ok_or_else(|| {
                    AppError::new(ErrorCode::NotFound, "no dash server connected").retryable(false)
                })?;
                let mut v = self.send(plan.method, &plan.path, &key, plan.body).await?;
                if channel == "dash:view" {
                    v["connected"] = json!(true);
                }
                Ok(v)
            }
        }
    }

    async fn connect_raw(&self, token: &str) -> Result<Value, AppError> {
        let url = format!("{ORIGIN}/v1/owner/setup");
        if !egress_ok(&url) {
            return Err(bad("the dash helper only talks to https://dash.qrtz.ai"));
        }
        let (status, body) = self.transport.send(Method::Post, &url, token, None).await?;
        if !(200..300).contains(&status) {
            return Err(status_error(status, &body));
        }
        let key = body["owner_key"]
            .as_str()
            .filter(|k| is_owner_key(k))
            .ok_or_else(|| {
                AppError::new(ErrorCode::Provider, "the dash server sent no owner key").retryable(false)
            })?;
        self.store
            .save(key)
            .map_err(|e| AppError::new(ErrorCode::Internal, format!("keychain: {e}")).retryable(false))?;
        Ok(json!({"connected": true, "origin": ORIGIN}))
    }

    /// One stdin line `{"id", "channel", "capability", "params"}` → one stdout line `{"id", "ok"}` or
    /// `{"id", "error"}`. Never panics; a line that isn't a known `dash:*` call with its contract capability is
    /// refused without any request.
    pub async fn handle_line(&self, line: &str) -> Value {
        if line.len() > MAX_LINE {
            return line_too_long();
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return json!({"id": Value::Null, "error": bad("invalid json").to_json()});
        };
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        let channel = v["channel"].as_str().unwrap_or("").to_owned();
        let channel = channel.as_str();
        let cap = v["capability"].as_str().unwrap_or("").to_owned();
        let cap = cap.as_str();
        let Some((_, want)) = CHANNELS.iter().find(|(c, _)| *c == channel) else {
            return json!({"id": id, "error": qrtz_ipc::no_handler(channel).to_json()});
        };
        if cap != *want {
            let e = AppError::new(ErrorCode::Forbidden, format!("{channel} needs {want}")).retryable(false);
            return json!({"id": id, "error": e.to_json()});
        }
        let mut v = v;
        let mut params = v.get_mut("params").map(Value::take).unwrap_or(Value::Null);
        let r = self.call(channel, &params).await;
        scrub_setup_token(&mut params);
        match r {
            Ok(r) => json!({"id": id, "ok": r}),
            Err(e) => json!({"id": id, "error": e.to_json()}),
        }
    }
}
