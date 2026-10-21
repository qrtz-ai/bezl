//! ADR 0228: Agent Dash's `dash:*` channels (the user's read-only view and controls) over one in-memory
//! [`qrtz_dash::Dash`]. The agent HTTP side is not served yet (no listener, no domain): `agent_request` is the
//! hook a future transport calls.
use qrtz_dash::{Dash, Pepper, Request, Response};
use qrtz_ipc::{no_handler, AppError, ErrorCode};
use serde_json::{json, Value};
use std::sync::Mutex;

pub const DASH_PREFIX: &str = "dash:";

/// Bozniak L4: the Rust-side flag. Off in release builds, so every `dash:*` call and every agent request fails
/// closed whatever the renderer does; on in debug builds (dev and tests). Flip with the renderer's DASH_ENABLED,
/// and only after ADR 0228's pre-listener findings are closed.
pub const DASH_ENABLED: bool = cfg!(debug_assertions);

pub struct DashPort {
    dash: Mutex<Dash>,
    enabled: bool,
    /// ADR 0231 amendment 1: the qrtz-dash-net helper, running only after the user opted in (`dash:connect`).
    net: tokio::sync::Mutex<Option<crate::dash_net::DashNet>>,
    /// QRT-298: the checked origin of the server `net` is connected to ([`accepted_origin`]); `None` = not connected.
    origin: Mutex<Option<String>>,
    helper_path: Option<std::path::PathBuf>,
}

impl Default for DashPort {
    fn default() -> Self {
        Self::with_enabled(DASH_ENABLED)
    }
}

fn off() -> AppError {
    AppError::new(ErrorCode::Unavailable, "dash is off in this build").retryable(false)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn bad(m: &str) -> AppError {
    AppError::new(ErrorCode::Validation, m).retryable(false)
}

/// QRT-298: the refusal of every code-issuing call while no dash server is connected. Nothing is issued.
pub const NOT_CONNECTED: &str = "not_connected";

pub fn not_connected() -> AppError {
    AppError::new(ErrorCode::Unavailable, NOT_CONNECTED).retryable(false)
}

/// The one production origin (twin of qrtz-dash-net's `ORIGIN` and packages/core DASH_ORIGIN).
pub const PROD_ORIGIN: &str = "https://dash.qrtz.ai";

/// `http://127.0.0.1:<1-65535>` exactly (the dev-local server; qrtz-dash-net's dev_origin rules).
fn is_loopback_dev_origin(o: &str) -> bool {
    o.strip_prefix("http://127.0.0.1:").is_some_and(|p| {
        !p.is_empty()
            && p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && !p.starts_with('0')
            && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
    })
}

/// QRT-298: the origins a status may report as connected. Release builds (and debug builds without `dev-local`):
/// [`PROD_ORIGIN`] only, so they can never report or accept a local origin. Debug + `dev-local` also takes the
/// loopback dev origin.
pub fn accepted_origin(o: &str) -> bool {
    o == PROD_ORIGIN || (cfg!(all(debug_assertions, feature = "dev-local")) && is_loopback_dev_origin(o))
}

/// QRT-284's `ENR-<id>.<secret>`: `ENR-`, 8 id chars, `.`, 17 secret chars, all from the dash server's code
/// alphabet (A-Z and 2-9 without I, L, O, U). The only code shape `dash:copy-prompt` will put in a prompt.
pub fn is_enroll_code(c: &str) -> bool {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";
    let b = c.as_bytes();
    let part = |r: &[u8]| r.iter().all(|x| ALPHABET.contains(x));
    b.len() == 30 && c.starts_with("ENR-") && b[12] == b'.' && part(&b[4..12]) && part(&b[13..30])
}

/// The text "Copy prompt" puts on the clipboard.
pub fn enroll_prompt(origin: &str, code: &str) -> zeroize::Zeroizing<String> {
    zeroize::Zeroizing::new(format!(
        "Read {origin}/v1/readme and enroll in my qrtz Dash with code {code}"
    ))
}

impl DashPort {
    pub fn with_enabled(enabled: bool) -> Self {
        Self::with_helper(enabled, crate::dash_net::default_helper_path())
    }

    /// `helper`: where qrtz-dash-net is (tests pass a fake).
    pub fn with_helper(enabled: bool, helper: Option<std::path::PathBuf>) -> Self {
        DashPort {
            dash: Mutex::new(Dash::new(Pepper::random())),
            enabled,
            net: tokio::sync::Mutex::new(None),
            origin: Mutex::new(None),
            helper_path: helper,
        }
    }

    fn spawn_helper(&self) -> Result<crate::dash_net::DashNet, AppError> {
        // Fails closed twice: only with dash on, and only from an explicit opt-in (the caller).
        if !self.enabled {
            return Err(off());
        }
        let path = self.helper_path.as_deref().ok_or_else(|| {
            AppError::new(ErrorCode::Unavailable, "the dash server needs a dev build").retryable(false)
        })?;
        crate::dash_net::DashNet::spawn(path)
    }

    fn capability(channel: &str) -> Result<String, AppError> {
        qrtz_ipc::contract()
            .channels
            .get(channel)
            .map(|c| c.capability.clone())
            .ok_or_else(|| no_handler(channel))
    }

    /// Zeroes a `setupToken` string in a payload in place (Bozniak L); the service calls it after `dash:connect`.
    pub fn scrub_setup_token(p: &mut Value) {
        if let Some(Value::String(t)) = p.get_mut("setupToken") {
            zeroize::Zeroize::zeroize(t);
        }
    }

    /// ADR 0231 amendment 1: every `dash:*` call. `dash:connect` is the opt-in that starts the helper; while it
    /// runs, the server answers; otherwise the in-memory `Dash` does. `dash:open-link` is always local.
    pub async fn call_async(&self, channel: &str, p: &Value) -> Result<Value, AppError> {
        if !self.enabled {
            return Err(off());
        }
        match channel {
            "dash:open-link" => self.call(channel, p),
            "dash:connect" | "dash:disconnect" => {
                let cap = Self::capability(channel)?;
                let mut net = self.net.lock().await;
                let mut h = match net.take() {
                    Some(h) => h,
                    None => self.spawn_helper()?,
                };
                let r = h.call(channel, &cap, p).await;
                self.set_origin(None);
                if channel == "dash:connect" {
                    if let Ok(v) = &r {
                        // QRT-298: keep only an origin this build accepts; anything else is not a connection.
                        let o = v["origin"]
                            .as_str()
                            .filter(|o| accepted_origin(o))
                            .map(str::to_owned);
                        if let Some(o) = o {
                            self.set_origin(Some(o));
                            *net = Some(h);
                            return r;
                        }
                        h.stop().await;
                        return Err(bad("the dash server's origin is not allowed in this build"));
                    }
                }
                h.stop().await;
                r
            }
            _ => {
                let mut net = self.net.lock().await;
                match net.as_mut() {
                    // QRT-298 (Bozniak): while connected, a code reaches an agent only through dash:copy-prompt, which
                    // writes the clipboard in Rust. dash:issue-code would hand the code to the renderer, so refuse it.
                    Some(_) if channel == "dash:issue-code" => Err(AppError::new(
                        ErrorCode::Unavailable,
                        "codes go out only through copy prompt while connected",
                    )),
                    Some(h) => {
                        let cap = Self::capability(channel)?;
                        let mut r = h.call(channel, &cap, p).await;
                        if r.as_ref().is_err_and(|e| e.message == "the dash helper stopped") {
                            if let Some(h) = net.take() {
                                h.stop().await;
                            }
                            self.set_origin(None);
                        }
                        // QRT-298: the status names the origin actually connected to (not the server's constant).
                        if channel == "dash:view" {
                            if let Ok(v) = r.as_mut() {
                                match self.connected_origin() {
                                    Some(o) if v.is_object() => v["origin"] = json!(o),
                                    _ => {
                                        if let Some(m) = v.as_object_mut() {
                                            m.remove("origin");
                                        }
                                    }
                                }
                            }
                        }
                        r
                    }
                    None => {
                        drop(net);
                        self.call(channel, p)
                    }
                }
            }
        }
    }

    fn set_origin(&self, o: Option<String>) {
        *self
            .origin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = o;
    }

    /// QRT-298: the connected server's origin, `None` when not connected.
    pub fn connected_origin(&self) -> Option<String> {
        self.origin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// QRT-298 `dash:copy-prompt`. Only when called: the connected server issues one fresh code, the prompt is
    /// built here and handed to `write` (the main-process clipboard). Returns only `{copied: true}`; the code is
    /// never returned, logged or kept (zeroed on drop). Not connected: [`not_connected`], nothing issued.
    pub async fn copy_prompt(&self, write: impl FnOnce(&str) -> bool) -> Result<Value, AppError> {
        if !self.enabled {
            return Err(off());
        }
        let mut net = self.net.lock().await;
        let (Some(h), Some(origin)) = (net.as_mut(), self.connected_origin()) else {
            return Err(not_connected());
        };
        if !accepted_origin(&origin) {
            return Err(not_connected());
        }
        let cap = Self::capability("dash:issue-code")?;
        let r = h.call("dash:issue-code", &cap, &Value::Null).await;
        let mut v = match r {
            Ok(v) => v,
            Err(e) => {
                if e.message == "the dash helper stopped" {
                    if let Some(h) = net.take() {
                        h.stop().await;
                    }
                    self.set_origin(None);
                }
                return Err(e);
            }
        };
        let code = zeroize::Zeroizing::new(v["code"].as_str().unwrap_or("").to_owned());
        if let Some(Value::String(c)) = v.get_mut("code") {
            zeroize::Zeroize::zeroize(c);
        }
        if !is_enroll_code(&code) {
            return Err(AppError::new(ErrorCode::Provider, "the dash server sent no code").retryable(true));
        }
        let prompt = enroll_prompt(&origin, &code);
        if !write(&prompt) {
            return Err(
                AppError::new(ErrorCode::Unavailable, "couldn't write the clipboard").retryable(true),
            );
        }
        Ok(json!({"copied": true}))
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn with<T>(&self, f: impl FnOnce(&mut Dash) -> T) -> T {
        let mut d = self
            .dash
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut d)
    }

    /// One agent request (the future HTTP listener's entry point).
    pub fn agent_request(&self, req: &Request) -> Response {
        if !self.enabled {
            return Response {
                status: 503,
                body: json!({"error": "dash is off"}),
            };
        }
        let now = now_ms();
        self.with(|d| d.handle(req, now))
    }

    pub fn call(&self, channel: &str, p: &Value) -> Result<Value, AppError> {
        if !self.enabled {
            return Err(off());
        }
        let now = now_ms();
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        match channel {
            "dash:view" => {
                // QRT-298: in memory = not connected, so no origin (never DASH_ORIGIN as a guess).
                let mut v = self.with(|d| d.view(now));
                if let Some(m) = v.as_object_mut() {
                    m.remove("origin");
                    m.insert("connected".into(), json!(false));
                }
                Ok(v)
            }
            // QRT-298: the legacy in-app code path is gone. No reachable server would accept its codes, so while
            // not connected nothing is minted; codes come only from the connected server (`dash:copy-prompt`).
            "dash:issue-code" | "dash:copy-prompt" => Err(not_connected()),
            "dash:rotate-codes" => {
                self.with(|d| d.rotate_codes(now));
                Ok(json!({"ok": true}))
            }
            "dash:review-agent" => {
                let id = s("agentId");
                let approve = p.get("approve").and_then(Value::as_bool).unwrap_or(false);
                let ok = self.with(|d| {
                    if approve {
                        d.approve(&id, now)
                    } else {
                        d.reject(&id, now)
                    }
                });
                Ok(json!({ "ok": ok }))
            }
            "dash:delete" => {
                let ok = match p.get("scope").and_then(Value::as_str) {
                    Some("widget") => self.with(|d| d.delete_widget(&s("agentId"), &s("key"), now)),
                    Some("agent") => self.with(|d| d.delete_agent(&s("agentId"), now)),
                    Some("dashboard") => {
                        self.with(|d| d.delete_dashboard(now));
                        true
                    }
                    _ => return Err(bad("scope is widget, agent or dashboard")),
                };
                Ok(json!({ "ok": ok }))
            }
            // Validated here; the caller (AppService) opens it through the shell, which checks again.
            "dash:open-link" => {
                let url = s("url");
                // 2048-character cap, as in the TS contract (Bozniak nit).
                if url.len() <= 2048 && qrtz_dash::is_https_url(&url) {
                    Ok(json!({ "url": url }))
                } else {
                    Err(bad("links open only if they are https"))
                }
            }
            _ => Err(no_handler(channel)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in helper (sh builtins only: it runs with a cleared environment, so no PATH; no network): answers every line with the `id` it was sent and echoes channel and
    /// capability back, so the test sees exactly what the app sends.
    #[cfg(unix)]
    fn fake_helper(dir: &std::path::Path) -> std::path::PathBuf {
        fake_helper_at(dir, "https://dash.qrtz.ai")
    }

    /// The same stand-in, answering with `origin` and a fixed code, and logging every channel to `calls.log`.
    #[cfg(unix)]
    fn fake_helper_at(dir: &std::path::Path, origin: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("qrtz-dash-net");
        // Written under a temp name and renamed, so no fd open for writing is inherited by a forked child (ETXTBSY).
        let tmp = dir.join("qrtz-dash-net.tmp");
        std::fs::write(
            &tmp,
            format!(
                r#"#!/bin/sh
while IFS= read -r l; do
  id=${{l#*\"id\":}}
  id=${{id%%[,\}}]*}}
  ch=${{l#*\"channel\":\"}}
  ch=${{ch%%\"*}}
  printf '%s\n' "$ch" >>'{log}'
  printf '{{"id":%s,"ok":{{"echo":%s,"env":"%s","origin":"{origin}","code":"ENR-AB23CD45.EFGH2345JKMN6789P","expiresAt":1}}}}\n' "$id" "$l" "${{CARGO_MANIFEST_DIR:-}}"
done
"#,
                log = dir.join("calls.log").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        p
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_helper_runs_only_when_on_and_opted_in_and_gets_only_dash_calls() {
        let d = tempfile::tempdir().unwrap();
        let fake = fake_helper(d.path());
        // Off: nothing spawns, every call fails closed.
        let off = DashPort::with_helper(false, Some(fake.clone()));
        assert!(off
            .call_async("dash:connect", &json!({"setupToken": "t"}))
            .await
            .is_err());
        assert!(off.net.lock().await.is_none());
        // On, not opted in: the in-memory dash answers; no helper.
        let port = DashPort::with_helper(true, Some(fake));
        let v = port.call_async("dash:view", &Value::Null).await.unwrap();
        assert!(v.get("echo").is_none());
        assert!(port.net.lock().await.is_none());
        // Opt in: the helper starts and gets the call with its contract capability.
        assert!(
            std::env::var_os("PATH").is_some(),
            "the test process has a PATH to leak"
        );
        let r = port
            .call_async("dash:connect", &json!({"setupToken": "t"}))
            .await
            .unwrap();
        assert_eq!(r["echo"]["channel"], "dash:connect");
        assert_eq!(r["echo"]["capability"], "dash.manage");
        assert_eq!(r["env"], "", "the helper gets a cleared environment");
        let v = port.call_async("dash:view", &Value::Null).await.unwrap();
        assert_eq!(v["echo"]["capability"], "dash.read");
        // open-link never goes to the helper.
        let l = port
            .call_async("dash:open-link", &json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert!(l.get("echo").is_none());
        // Disconnect stops it; the in-memory dash answers again.
        port.call_async("dash:disconnect", &Value::Null).await.unwrap();
        assert!(port.net.lock().await.is_none());
        let v = port.call_async("dash:view", &Value::Null).await.unwrap();
        assert!(v.get("echo").is_none());
    }

    #[cfg(unix)]
    fn calls(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn only_the_production_origin_is_accepted_unless_dev_local() {
        assert!(accepted_origin("https://dash.qrtz.ai"));
        let dev = cfg!(all(debug_assertions, feature = "dev-local"));
        assert_eq!(accepted_origin("http://127.0.0.1:8799"), dev);
        for o in [
            "http://dash.qrtz.ai",
            "https://dash.qrtz.ai/",
            "https://dash.qrtz.ai.evil.example",
            "http://localhost:8799",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://127.0.0.1:65536",
            "http://127.0.0.1:8799/",
            "http://127.0.0.1:8799@evil",
            "",
        ] {
            assert!(!accepted_origin(o), "{o}");
        }
    }

    #[test]
    fn the_prompt_names_the_origin_and_a_valid_code() {
        let p = enroll_prompt("http://127.0.0.1:8799", "ENR-AB23CD45.EFGH2345JKMN6789P");
        assert_eq!(
            p.as_str(),
            "Read http://127.0.0.1:8799/v1/readme and enroll in my qrtz Dash with code ENR-AB23CD45.EFGH2345JKMN6789P"
        );
        assert!(is_enroll_code("ENR-AB23CD45.EFGH2345JKMN6789P"));
        for c in [
            "ENR-AB12-CD34",                   // the old in-app shape: no server issues it
            "TST-AB23CD45.EFGH2345JKMN6789P",  // local test codes never go in a prompt
            "ENR-ab23cd45.EFGH2345JKMN6789P",  // lower case
            "ENR-AB23CD45EFGH2345JKMN6789P",   // no dot
            "ENR-AB23CD45.EFGH2345JKMN6789PQ", // secret too long
            "ENR-AB23CD4.EFGH2345JKMN6789PQ",  // id too short
            "ENR-AB23CD45.EFGH2345JKMN6789I",  // I is not in the alphabet
            "ENR-AB23CD40.EFGH2345JKMN6789P",  // nor 0
            "ENR-AB23CD45.EFGH2345JKMN6789P ",
            "",
        ] {
            assert!(!is_enroll_code(c), "{c}");
        }
    }

    #[tokio::test]
    async fn not_connected_status_has_no_origin_and_nothing_issues_a_code() {
        let port = DashPort::with_helper(true, None);
        let v = port.call_async("dash:view", &Value::Null).await.unwrap();
        assert!(v.get("origin").is_none(), "{v}");
        assert_eq!(v["connected"], false);
        let def = &qrtz_ipc::contract().channels["dash:view"];
        assert!(def.response.is_valid(&v), "{v}");
        // The legacy in-app path is gone: not_connected, and the in-memory dash minted nothing.
        let before = port.with(|d| d.view(0)).to_string();
        let e = port
            .call_async("dash:issue-code", &Value::Null)
            .await
            .unwrap_err();
        assert_eq!(e.message, NOT_CONNECTED);
        let mut wrote = false;
        let e = port
            .copy_prompt(|_| {
                wrote = true;
                true
            })
            .await
            .unwrap_err();
        assert_eq!(e.message, NOT_CONNECTED);
        assert!(!wrote, "nothing reaches the clipboard");
        assert_eq!(port.with(|d| d.view(0)).to_string(), before, "no code minted");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connected_status_names_the_origin_and_copy_prompt_issues_only_when_called() {
        let d = tempfile::tempdir().unwrap();
        let port = DashPort::with_helper(true, Some(fake_helper(d.path())));
        port.call_async("dash:connect", &json!({"setupToken": "t"}))
            .await
            .unwrap();
        assert_eq!(port.connected_origin().as_deref(), Some(PROD_ORIGIN));
        // "Render": status reads issue nothing.
        for _ in 0..3 {
            let v = port.call_async("dash:view", &Value::Null).await.unwrap();
            assert_eq!(v["origin"], PROD_ORIGIN);
        }
        assert!(!calls(d.path()).iter().any(|c| c == "dash:issue-code"));
        // QRT-298 (Bozniak): connected, the renderer's dash:issue-code is refused and never reaches the server.
        let e = port
            .call_async("dash:issue-code", &Value::Null)
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Unavailable);
        assert!(!calls(d.path()).iter().any(|c| c == "dash:issue-code"));
        let mut clip = String::new();
        let r = port
            .copy_prompt(|t| {
                clip = t.to_owned();
                true
            })
            .await
            .unwrap();
        assert_eq!(r, json!({"copied": true}), "only copied comes back");
        assert!(!r.to_string().contains("ENR-"));
        assert_eq!(
            clip,
            "Read https://dash.qrtz.ai/v1/readme and enroll in my qrtz Dash with code ENR-AB23CD45.EFGH2345JKMN6789P"
        );
        assert_eq!(
            calls(d.path()).iter().filter(|c| *c == "dash:issue-code").count(),
            1,
            "one fresh code per call"
        );
        port.call_async("dash:disconnect", &Value::Null).await.unwrap();
        assert!(port.connected_origin().is_none());
        let e = port.copy_prompt(|_| true).await.unwrap_err();
        assert_eq!(e.message, NOT_CONNECTED);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_local_origin_is_refused_without_dev_local() {
        let d = tempfile::tempdir().unwrap();
        let port = DashPort::with_helper(true, Some(fake_helper_at(d.path(), "http://127.0.0.1:8799")));
        let r = port.call_async("dash:connect", &json!({"setupToken": "t"})).await;
        if cfg!(all(debug_assertions, feature = "dev-local")) {
            assert!(r.is_ok());
            assert_eq!(port.connected_origin().as_deref(), Some("http://127.0.0.1:8799"));
        } else {
            assert!(r.is_err());
            assert!(port.connected_origin().is_none());
            assert!(port.net.lock().await.is_none());
        }
    }

    #[tokio::test]
    async fn without_a_helper_binary_connect_fails_and_nothing_changes() {
        let port = DashPort::with_helper(true, None);
        assert!(port.call_async("dash:connect", &json!({})).await.is_err());
        assert!(port.call_async("dash:view", &Value::Null).await.is_ok());
    }

    #[test]
    fn off_fails_closed_for_ipc_and_agents() {
        let port = DashPort::with_enabled(false);
        for ch in [
            "dash:view",
            "dash:issue-code",
            "dash:rotate-codes",
            "dash:open-link",
        ] {
            assert_eq!(
                port.call(ch, &Value::Null).unwrap_err().code,
                ErrorCode::Unavailable,
                "{ch}"
            );
        }
        let r = port.agent_request(
            &Request::new("POST", "/v1/enroll").json(json!({"code": "ENR-AAAA-AAAA", "name": "x"})),
        );
        assert_eq!(r.status, 503);
    }

    #[test]
    fn open_link_takes_https_only() {
        let port = DashPort::with_enabled(true);
        assert!(port
            .call("dash:open-link", &json!({"url": "https://qrtz.ai/x"}))
            .is_ok());
        for u in [
            "http://qrtz.ai",
            "javascript:alert(1)",
            "https://a.com:x",
            "file:///etc/passwd",
            format!("https://qrtz.ai/{}", "a".repeat(2048)).as_str(),
        ] {
            assert!(port.call("dash:open-link", &json!({"url": u})).is_err(), "{u}");
        }
    }

    #[test]
    fn the_user_side_round_trips_and_the_view_holds_no_secrets() {
        let port = DashPort::with_enabled(true);
        // QRT-298: the renderer can no longer mint in-memory codes; the agent side is driven directly here.
        let code = port.with(|d| d.issue_code(now_ms())).unwrap();
        let r = port.agent_request(
            &Request::new("POST", "/v1/enroll").json(json!({"code": code, "name": "grok bot"})),
        );
        assert_eq!(r.status, 201);
        let id = r.body["agent_id"].as_str().unwrap().to_owned();
        let token = r.body["agent_token"].as_str().unwrap().to_owned();
        assert_eq!(
            port.call("dash:review-agent", &json!({"agentId": id, "approve": true}))
                .unwrap()["ok"],
            true
        );
        let view = port.call("dash:view", &Value::Null).unwrap().to_string();
        assert!(!view.contains(&token) && !view.contains(&code));
        let v: Value = serde_json::from_str(&view).unwrap();
        let def = &qrtz_ipc::contract().channels["dash:view"];
        assert!(def.response.is_valid(&v), "{v}");
        assert_eq!(
            port.call("dash:delete", &json!({"scope": "agent", "agentId": id}))
                .unwrap()["ok"],
            true
        );
        let r = port.agent_request(
            &Request::new("POST", "/v1/heartbeat")
                .bearer(&token)
                .json(json!({"status": "idle"})),
        );
        assert_eq!(r.status, 410);
    }
}
