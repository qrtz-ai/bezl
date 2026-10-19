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
                if channel == "dash:connect" && r.is_ok() {
                    *net = Some(h);
                } else {
                    h.stop().await;
                }
                r
            }
            _ => {
                let mut net = self.net.lock().await;
                match net.as_mut() {
                    Some(h) => {
                        let cap = Self::capability(channel)?;
                        let r = h.call(channel, &cap, p).await;
                        if r.as_ref().is_err_and(|e| e.message == "the dash helper stopped") {
                            if let Some(h) = net.take() {
                                h.stop().await;
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
            "dash:view" => Ok(self.with(|d| d.view(now))),
            "dash:issue-code" => self
                .with(|d| d.issue_code(now))
                .map(|code| json!({"code": code, "expiresAt": now + qrtz_dash::ENROLL_TTL_MS}))
                .ok_or_else(|| bad("this dashboard was deleted")),
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
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("qrtz-dash-net");
        std::fs::write(
            &p,
            r#"#!/bin/sh
while IFS= read -r l; do
  id=${l#*\"id\":}
  id=${id%%[,\}]*}
  printf '{"id":%s,"ok":{"echo":%s,"env":"%s"}}\n' "$id" "$l" "${QRTZ_PROBE:-}"
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        std::env::set_var("QRTZ_PROBE", "leaked");
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
        let code = port.call("dash:issue-code", &Value::Null).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_owned();
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
