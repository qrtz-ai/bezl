//! Seams to the other tracks. Each port takes a validated channel payload and returns the response JSON,
//! so the adapter onto the real crates is the one-liner table in docs/rust-core-api.md §4.3:
//!   RuntimePort → qrtz-lifecycle `NoteRuntime` (feature "runtime")
//!   StoragePort → qrtz-storage `StorageHandle`
//!   TalkPort    → the Rust TalkService port (talk worker's TS service today)
//! Until those land, the `Stub*` impls answer with the golden idle samples and refuse actions with a
//! retryable E_UNAVAILABLE, so the Tauri UI boots, renders empty states and shows honest errors.
use async_trait::async_trait;
use qrtz_helper_bridge::HelperStatus;
use qrtz_ipc::{idle_response, AppError, ErrorCode};
use serde_json::Value;
use std::sync::Mutex;

/// Channels the recording runtime owns (runtime.* in §4.3).
pub const RUNTIME_CHANNELS: &[&str] = &[
    "note:session",
    "note:start",
    "note:stop",
    "note:dismiss",
    "note:pause",
    "note:pipeline",
    "note:retry-step",
    "note:live-transcript",
    "note:resummarize",
    "note:set-template",
    "note:add-note",
    "permissions:get",
    "permissions:request",
];
/// Channels served from storage directly (plus shell-side tombstones / export / backup zip).
pub const STORAGE_CHANNELS: &[&str] = &[
    "note:list",
    "note:get",
    "note:list-notes",
    "note:update-note",
    "note:delete-note",
    "note:rename-speaker",
    "note:delete",
    "note:undo-delete",
    "note:export",
    "db:health",
    "data:export-backup",
    "data:import-backup",
];
/// Channels the app service itself answers.
pub const APP_CHANNELS: &[&str] = &[
    "app:get-info",
    "config:get",
    "config:set",
    "note:set-auto-record",
    "secrets:set",
    "secrets:list",
    "helper:status",
    "hud:set-expanded",
    "note:chat",
];
pub const TALK_PREFIX: &str = "talk:";

#[async_trait]
pub trait RuntimePort: Send + Sync {
    async fn call(&self, channel: &str, payload: Value) -> Result<Value, AppError>;
    /// Tray / global shortcut record toggle (runtime.toggle()).
    async fn toggle(&self) -> Result<Value, AppError>;
    /// A validated helper → host line for the recording runtime (it narrows to `HelperInput`).
    fn helper_line(&self, msg: &Value);
    fn helper_status(&self, status: &HelperStatus);
}

#[async_trait]
pub trait StoragePort: Send + Sync {
    async fn call(&self, channel: &str, payload: Value) -> Result<Value, AppError>;
}

#[async_trait]
pub trait TalkPort: Send + Sync {
    async fn call(&self, channel: &str, payload: Value) -> Result<Value, AppError>;
    /// Talk helper lines (`dictation.*`, `insert.*`, `hotkey`) and every `hello`: a hello from a newer helper while
    /// a dictation is active means the helper restarted under it (T1/T2 helper-lost path).
    fn helper_line(&self, msg: &Value);
    /// Supervisor status. Not running or starting while a dictation is active → end it gracefully with reason
    /// `helper-lost` and release the started/stopped/insert waiters (mirrors Electron `TalkService.onHelperLost`).
    fn helper_status(&self, _s: &HelperStatus) {}
    /// Global dictation hotkey (push-to-talk down/up or toggle).
    async fn toggle(&self) -> Result<Value, AppError>;
}

/// Window-level effects owned by the Tauri shell.
pub trait ShellHooks: Send + Sync {
    fn set_hud_expanded(&self, expanded: bool) -> bool;
}

pub struct NoShell;
impl ShellHooks for NoShell {
    fn set_hud_expanded(&self, expanded: bool) -> bool {
        expanded
    }
}

fn pending(track: &str, channel: &str) -> AppError {
    AppError::new(
        ErrorCode::Unavailable,
        format!("{channel} is not available in the Tauri build yet ({track})"),
    )
    .retryable(false)
}

fn idle_or_pending(track: &str, channel: &str) -> Result<Value, AppError> {
    idle_response(channel).ok_or_else(|| pending(track, channel))
}

/// Idle runtime: nothing recording; permissions mirror the last `permissions.status` the helper sent.
#[derive(Default)]
pub struct StubRuntime {
    permissions: Mutex<Option<Value>>,
    helper: Mutex<Option<HelperStatus>>,
}

#[async_trait]
impl RuntimePort for StubRuntime {
    async fn call(&self, channel: &str, _payload: Value) -> Result<Value, AppError> {
        match channel {
            "permissions:get" => {
                let mut v = idle_response("permissions:get").unwrap_or(Value::Null);
                if let Some(Value::Object(p)) = self.permissions.lock().unwrap().clone() {
                    for (k, s) in p {
                        v[k] = s;
                    }
                }
                Ok(v)
            }
            "note:session" | "note:dismiss" => {
                let mut v = idle_or_pending("qrtz-lifecycle runtime", channel)?;
                if let Some(h) = self.helper.lock().unwrap().as_ref() {
                    use qrtz_helper_bridge::HelperState::*;
                    v["helper"] = match h.state {
                        Running | Starting => "ok",
                        Stopped => "simulated",
                        Backoff | Failed => "down",
                    }
                    .into();
                }
                Ok(v)
            }
            _ => idle_or_pending("qrtz-lifecycle runtime", channel),
        }
    }
    async fn toggle(&self) -> Result<Value, AppError> {
        Err(pending("qrtz-lifecycle runtime", "toggle"))
    }
    fn helper_line(&self, msg: &Value) {
        if msg["type"] == "permissions.status" {
            *self.permissions.lock().unwrap() = Some(msg["status"].clone());
        }
    }
    fn helper_status(&self, status: &HelperStatus) {
        *self.helper.lock().unwrap() = Some(status.clone());
    }
}

pub struct StubStorage;
#[async_trait]
impl StoragePort for StubStorage {
    async fn call(&self, channel: &str, _payload: Value) -> Result<Value, AppError> {
        match channel {
            "db:health" => Ok(serde_json::json!({"ok": false, "schemaVersion": 0, "fts5": false})),
            _ => idle_or_pending("qrtz-storage", channel),
        }
    }
}

pub struct StubTalk;
#[async_trait]
impl TalkPort for StubTalk {
    async fn call(&self, channel: &str, _payload: Value) -> Result<Value, AppError> {
        idle_or_pending("talk service port", channel)
    }
    fn helper_line(&self, _msg: &Value) {}
    async fn toggle(&self) -> Result<Value, AppError> {
        Err(pending("talk service port", "toggle"))
    }
}
