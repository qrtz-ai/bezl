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
#![forbid(unsafe_code)]
pub mod legacy_sweep;
use qrtz_ipc::{AppError, ErrorCode};
use serde_json::{json, Value};
use zeroize::{Zeroize, Zeroizing};

/// The one origin this process may reach (release builds: always).
pub const ORIGIN: &str = "https://dash.qrtz.ai";
pub const KEYCHAIN_SERVICE: &str = "ai.qrtz.dash-net";
/// The Keychain service while the dev origin override is on (Bozniak item 5): never the real item.
pub const DEV_KEYCHAIN_SERVICE: &str = "qrtz-dash-dev";
/// The env var the dev-only origin override reads (docs/dash-local.md). FOR BOZNIAK REVIEW.
pub const DEV_ORIGIN_VAR: &str = "QRTZ_DASH_ORIGIN";
/// `true` only in debug builds with the `dev-local` feature. Everywhere else the override code is not compiled in.
pub const DEV_OVERRIDE_COMPILED: bool = cfg!(all(debug_assertions, feature = "dev-local"));

/// The dev-only loopback origin: compiled only into debug + dev-local builds (src/dev_origin.rs).
#[cfg(all(debug_assertions, feature = "dev-local"))]
pub mod dev_origin;

#[cfg(all(debug_assertions, feature = "dev-local"))]
fn dev_origin() -> Result<Option<&'static str>, ()> {
    dev_origin::read()
}

/// Release builds (or no `dev-local`): there is no override path at all, and the helper fails closed: if the
/// variable is set anyway, `Err` (main refuses to start) instead of silently talking to the real origin.
#[cfg(not(all(debug_assertions, feature = "dev-local")))]
fn dev_origin() -> Result<Option<&'static str>, ()> {
    if std::env::var_os(DEV_ORIGIN_VAR).is_some() {
        Err(())
    } else {
        Ok(None)
    }
}

/// Startup check for main: `false` if a dev override is set but invalid, or set at all in a build without it.
pub fn origin_config_ok() -> bool {
    dev_origin().is_ok()
}

/// The origin this process talks to: [`ORIGIN`], or the checked dev loopback origin.
pub fn origin() -> &'static str {
    match dev_origin() {
        Ok(Some(o)) => o,
        _ => ORIGIN,
    }
}

/// True when the dev loopback origin is in use.
pub fn dev_origin_active() -> bool {
    matches!(dev_origin(), Ok(Some(_)))
}

/// The origin an owner key is bound to, stored with its Keychain item (the item's comment, which is also part of
/// every lookup, so a key saved for one origin is never found, and so never sent, under another). The service name
/// differs too ([`keychain_service`]). The origin is fixed for the life of the process.
pub fn key_origin_tag() -> String {
    format!("qrtz-dash origin {}", origin())
}

/// The Keychain service for the current origin.
pub fn keychain_service() -> &'static str {
    if dev_origin_active() {
        DEV_KEYCHAIN_SERVICE
    } else {
        KEYCHAIN_SERVICE
    }
}
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
/// With the dev override on, the same rules with the dev origin in place of [`ORIGIN`] (and nothing else).
pub fn egress_ok(url: &str) -> bool {
    let Some(path) = url.strip_prefix(origin()) else {
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
    /// The owner key, or `None`. A store error reads as `None`, never as an older key ([`KeyStore::try_load`]).
    fn load(&self) -> Option<String> {
        self.try_load().ok().flatten()
    }
    /// The owner key, `Ok(None)` if there is none, or the store's error (QRT-264: a keychain error on the pending
    /// key is surfaced, not skipped in favour of the older main key).
    fn try_load(&self) -> Result<Option<String>, String>;
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

/// One Keychain account, raw: what [`TwoPhase`] builds a safe save on (the Keychain on macOS, a fake in tests).
pub trait RawKeychain: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, String>;
    /// Stores `secret` under `account`: creates the item, or replaces its data if `account` already holds one
    /// (the macOS Keychain adds, then updates on a duplicate). [`TwoPhase`] never relies on it failing: it deletes
    /// the account first, so it only ever creates (an old item can't keep weaker attributes).
    fn add(&self, account: &str, secret: &[u8]) -> Result<(), String>;
    /// Deletes the item; a missing item is `Ok`.
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// Where a new owner key waits until it has replaced the old one (Bozniak M6).
pub const KEYCHAIN_PENDING_ACCOUNT: &str = "owner-key.pending";
/// How many times the final add under [`KEYCHAIN_ACCOUNT`] is tried before the key is left under the pending account.
pub const SAVE_ATTEMPTS: usize = 3;

/// Bozniak M6: a save that never leaves the helper without a readable owner key.
///
/// 1. the new key is added under [`KEYCHAIN_PENDING_ACCOUNT`] and read back; on any failure the old item is untouched;
/// 2. only then is the old item deleted (still delete-then-add, so an old item can't keep weaker attributes), and the
///    key is added under [`KEYCHAIN_ACCOUNT`], retried [`SAVE_ATTEMPTS`] times, and read back;
/// 3. only then is the pending item deleted. If step 2 keeps failing the pending item stays, and the next save first
///    moves it into [`KEYCHAIN_ACCOUNT`], so the key is never lost.
/// 4. if deleting the old item fails, the pending item is kept (the setup token is one-time, so it may hold the new
///    dashboard's only key) and the save fails with a clear error.
///
/// Bozniak M6 round 3 (QRT-260): a pending key is never older than the main one, so the pending copy is never
/// discarded while it is valid. [`KeyStore::load`] prefers a valid pending key, and every save first recovers it into
/// [`KEYCHAIN_ACCOUNT`] (read back) when it differs from main or main is empty or invalid. This also covers a crash
/// between the pending read-back and the old-key delete.
pub struct TwoPhase<R: RawKeychain>(pub R);

fn holds(r: &dyn RawKeychain, account: &str, key: &str) -> bool {
    matches!(r.get(account), Ok(Some(b)) if b == key.as_bytes())
}

fn valid(b: Vec<u8>) -> Option<String> {
    String::from_utf8(b).ok().filter(|k| is_owner_key(k))
}

impl<R: RawKeychain> TwoPhase<R> {
    /// Moves a valid pending key into the main account (read back) and only then deletes pending.
    /// An invalid pending item is just removed. Never deletes a valid pending key it has not copied.
    fn recover(&self) -> Result<(), String> {
        let r = &self.0;
        let Some(pending) = r.get(KEYCHAIN_PENDING_ACCOUNT)?.and_then(valid) else {
            return r.delete(KEYCHAIN_PENDING_ACCOUNT);
        };
        let main_raw = r.get(KEYCHAIN_ACCOUNT)?;
        let main_exists = main_raw.is_some();
        // An invalid main counts as missing.
        let main = main_raw.and_then(valid);
        if main.as_deref() != Some(pending.as_str()) {
            if main_exists {
                r.delete(KEYCHAIN_ACCOUNT).map_err(|e| {
                    format!("could not replace the old owner key ({e}); the newer key is kept under the pending account")
                })?;
            }
            r.add(KEYCHAIN_ACCOUNT, pending.as_bytes())
                .map_err(|e| format!("{e} (the newer owner key is kept under the pending account)"))?;
            if !holds(r, KEYCHAIN_ACCOUNT, &pending) {
                let _ = r.delete(KEYCHAIN_ACCOUNT);
                return Err(
                    "the newer owner key did not read back; it is kept under the pending account".into(),
                );
            }
        }
        r.delete(KEYCHAIN_PENDING_ACCOUNT)
    }
}

impl<R: RawKeychain> KeyStore for TwoPhase<R> {
    fn try_load(&self) -> Result<Option<String>, String> {
        // Pending is never older than main, so a valid pending key wins. An error reading pending is returned,
        // never skipped: falling through to main could hand back the older key (QRT-264).
        if let Some(k) = self.0.get(KEYCHAIN_PENDING_ACCOUNT)?.and_then(valid) {
            return Ok(Some(k));
        }
        Ok(self.0.get(KEYCHAIN_ACCOUNT)?.and_then(valid))
    }
    fn save(&self, key: &str) -> Result<(), String> {
        let r = &self.0;
        self.recover()?;
        r.add(KEYCHAIN_PENDING_ACCOUNT, key.as_bytes())?;
        if !holds(r, KEYCHAIN_PENDING_ACCOUNT, key) {
            let _ = r.delete(KEYCHAIN_PENDING_ACCOUNT);
            return Err("the new owner key did not read back; the old one is kept".into());
        }
        // Past here the new key is safe under the pending account and load returns it.
        if let Err(e) = r.delete(KEYCHAIN_ACCOUNT) {
            // Never discard pending: the setup token is one-time, so it may be the new dashboard's only key.
            return Err(format!(
                "could not replace the old owner key ({e}); the new key is kept under the pending account and will be moved on the next save"
            ));
        }
        let mut last = String::new();
        for _ in 0..SAVE_ATTEMPTS {
            match r.add(KEYCHAIN_ACCOUNT, key.as_bytes()) {
                Ok(()) if holds(r, KEYCHAIN_ACCOUNT, key) => {
                    // Main holds the same key now, so a leftover pending copy is harmless.
                    let _ = r.delete(KEYCHAIN_PENDING_ACCOUNT);
                    return Ok(());
                }
                Ok(()) => {
                    last = "the owner key did not read back".into();
                    let _ = r.delete(KEYCHAIN_ACCOUNT);
                }
                Err(e) => last = e,
            }
        }
        // Kept under the pending account: load still finds it, and the next save moves it into the main
        // account before doing anything else. The setup token is one-time, so nothing re-runs the connect.
        Err(format!(
            "{last} (the key is kept and will be moved on the next save)"
        ))
    }
    fn delete(&self) -> Result<(), String> {
        self.0.delete(KEYCHAIN_ACCOUNT)?;
        self.0.delete(KEYCHAIN_PENDING_ACCOUNT)
    }
}

#[cfg(target_os = "macos")]
fn keychain_options(
    account: &str,
    write: bool,
) -> Result<security_framework::passwords::PasswordOptions, String> {
    use security_framework::passwords::PasswordOptions;
    let mut o = PasswordOptions::new_generic_password(keychain_service(), account);
    o.set_comment(&key_origin_tag());
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
impl RawKeychain for KeychainStore {
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, String> {
        const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
        match security_framework::passwords::generic_password(keychain_options(account, false)?) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn add(&self, account: &str, secret: &[u8]) -> Result<(), String> {
        security_framework::passwords::set_generic_password_options(secret, keychain_options(account, true)?)
            .map_err(|e| e.to_string())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
        match security_framework::passwords::delete_generic_password_options(keychain_options(
            account, false,
        )?) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// No Keychain: the server can't be used on this platform (the key would have to go to disk).
pub struct NoStore;

impl KeyStore for NoStore {
    fn try_load(&self) -> Result<Option<String>, String> {
        Ok(None)
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
    fn try_load(&self) -> Result<Option<String>, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.clone())
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
        Box::new(TwoPhase(KeychainStore))
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
            // Plain http only for the checked dev loopback origin (debug + dev-local); release: https only.
            .https_only(!dev_origin_active())
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

fn keychain_error(e: String) -> AppError {
    AppError::new(ErrorCode::Internal, format!("keychain: {e}")).retryable(false)
}

impl Helper {
    fn load_key(&self) -> Result<Option<String>, AppError> {
        self.store.try_load().map_err(keychain_error)
    }

    async fn send(
        &self,
        m: Method,
        path: &str,
        bearer: &str,
        body: Option<Value>,
    ) -> Result<Value, AppError> {
        let url = format!("{}{path}", origin());
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
                if self.load_key()?.is_some() {
                    Ok(json!({"connected": true, "origin": origin()}))
                } else {
                    Err(AppError::new(ErrorCode::NotFound, "no dash server connected yet").retryable(false))
                }
            }
            "dash:disconnect" => {
                self.store.delete().map_err(keychain_error)?;
                Ok(json!({"ok": true}))
            }
            _ => {
                let plan = plan(channel, p)?;
                let key = self.load_key()?.ok_or_else(|| {
                    AppError::new(ErrorCode::NotFound, "no dash server connected").retryable(false)
                })?;
                let mut v = self.send(plan.method, &plan.path, &key, plan.body).await?;
                if channel == "dash:view" && v.is_object() {
                    v["connected"] = json!(true);
                    // QRT-298: the origin this helper actually talks to, not the server's own constant.
                    v["origin"] = json!(origin());
                }
                Ok(v)
            }
        }
    }

    async fn connect_raw(&self, token: &str) -> Result<Value, AppError> {
        let url = format!("{}/v1/owner/setup", origin());
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
        if let Err(e) = self.store.save(key) {
            // QRT-264: say whether the new key still loads (it does once the pending copy is stored and read back).
            let still_loads = matches!(self.store.try_load(), Ok(Some(k)) if k == key);
            let next = if still_loads {
                "the new owner key is stored and still loads, so this dashboard stays connected"
            } else {
                "the new owner key was not stored, so this dashboard is not connected"
            };
            return Err(keychain_error(format!("{e}; {next}")));
        }
        Ok(json!({"connected": true, "origin": origin()}))
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
