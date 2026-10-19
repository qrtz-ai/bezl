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
        DashPort {
            dash: Mutex::new(Dash::new(Pepper::random())),
            enabled,
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
