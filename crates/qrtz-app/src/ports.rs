//! Seams to the other tracks. Each port takes a validated channel payload and returns the response JSON,
//! so the adapter onto the real crates is the one-liner table in docs/rust-core-api.md §4.3:
//!   RuntimePort → qrtz-lifecycle `NoteRuntime` (feature "runtime")
//!   StoragePort → qrtz-storage `StorageHandle`
//!   TalkPort    → Bozniak's qrtz-talk service (routing only here)
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
    "note:keep-recording",
    // The runtime owns the full delete (pipeline, rows, audio; refuses the meeting being recorded).
    "note:delete",
    // Tombstones live in the runtime (N17).
    "note:undo-delete",
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
    "note:export",
    "db:health",
    "data:export-backup",
    "data:import-backup",
];
/// Channels the app service itself answers.
pub const APP_CHANNELS: &[&str] = &[
    "app:get-info",
    // Audit P0-4: the third-party notices, built into the binary (settings → "open-source licenses").
    "app:licenses",
    "config:get",
    "config:set",
    "note:set-auto-record",
    // ADR 0146 M1.4: each module's settings page.
    "module:get-settings",
    "module:set-settings",
    "helper:status",
    "hud:set-expanded",
    // ADR 0083: the local log.
    "log:write",
    "logs:export",
    // ADR 0161: the module manager's on/off switch (packages.manage; writes only `modules.<id>.enabled`).
    "packages:set-enabled",
    // System Settings at a permission's pane, and "restart qrtz" for a grant that needs one.
    "permissions:open-settings",
    "app:restart",
    // ADR 0184: the permissions onboarding is done (the host writes `onboarding.done`).
    "onboarding:finish",
];
pub const TALK_PREFIX: &str = "talk:";
/// ctrl (ADR 0103): tasks, schedules and run history (`qrtz_ctrl::service::CtrlService`).
pub const CTRL_PREFIX: &str = "ctrl:";
/// qrtz snap (ADR 0159): what qrtz-snap last reported (`crate::snap::SnapHost`).
pub const SNAP_CHANNELS: &[&str] = &["snap:status", "snap:request-access"];
/// qrtz speech setup (settings "speech" and the modules switches; ADR 0046 §7, ADR 0163).
pub const SETUP_CHANNELS: &[&str] = &[
    "speech:status",
    "speech:check",
    "speech:download",
    "speech:pause",
    "speech:resume",
    "speech:delete",
];

/// ADR 0181: qrtz's own update (settings → qrtz, the sidebar dot): `crate::self_update`.
pub const APP_UPDATE_PREFIX: &str = "app-update:";

/// ADR 0217: nano's Apple Calendar (`calendar:*`): `crate::calendar`.
pub const CALENDAR_PREFIX: &str = "calendar:";

/// The package manager (ADR 0157; the Extensions screen): `crate::packages_ipc`.
pub const PACKAGES_PREFIX: &str = "packages:";

/// `packages:*` (list, refresh, plan, apply, prune).
#[async_trait]
pub trait PackagesPort: Send + Sync {
    async fn call(&self, channel: &str, payload: Value) -> Result<Value, AppError>;
}

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

/// Speech module download/update/delete (`crate::setup::Setup`).
#[async_trait]
pub trait SetupPort: Send + Sync {
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
    /// `config:set` happened: push `dictation.configure` (Electron
    /// `config.onChange(() => talk.configure())`).
    fn config_changed(&self) {}
    /// ADR 0092: a pill flash for the helper's native pill (`pill.flash`).
    fn pill_flash(&self, _kind: &str, _message: &str) {}
}

/// OS notifications the core asks the shell for. Not IPC events: the shell decides whether a window is focused
/// (in-app UI covers that case) and what a click does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// N29: every capture source of this meeting went silent (`noAudio` turned on) and `note.notifyNoAudio` is on.
    /// Edge-triggered: once per meeting until `noAudio` clears.
    NoAudio { meeting_id: String },
    /// A meeting was detected and auto-record is off (`CoreEvent::DetectedPrompt`); a click starts recording.
    MeetingDetected { title: String, platform: String },
    /// ctrl's confirm gate (ADR 0111): a run waits for the user's ok before something that can't be undone. A
    /// banner only (it never activates qrtz or takes focus); the answer happens on the ctrl view's card.
    CtrlConfirm { task_id: String, run_id: String },
}

/// Window-level effects owned by the Tauri shell.
pub trait ShellHooks: Send + Sync {
    fn set_hud_expanded(&self, expanded: bool) -> bool;
    /// Raise an OS notification (Tauri shell); headless builds ignore it.
    fn notify(&self, _n: Notice) {}
    /// `logs:export`: where to save the logs (the save dialog; blocking, called off the async runtime). `None` =
    /// canceled, or no dialog in this build.
    fn logs_save_path(&self, _default_name: &str) -> Option<std::path::PathBuf> {
        None
    }
    /// `logs:export {reveal}`: show `dir` in Finder (blocking, off the async runtime). `false` = couldn't.
    fn reveal(&self, _dir: &std::path::Path) -> bool {
        false
    }
    /// `app:restart`: quit and relaunch qrtz (soon after answering). `false` = this build can't.
    fn restart(&self) -> bool {
        false
    }
    /// ADR 0228: open an https link in the default browser (the Tauri opener). `false` = refused or no shell.
    /// Implementations must check `qrtz_dash::is_https_url` themselves.
    fn open_https(&self, _url: &str) -> bool {
        false
    }
    /// QRT-298 `dash:copy-prompt`: put text on the main-process clipboard. `false` = couldn't, or no shell.
    /// Implementations must never log the text (it holds an enrollment code).
    fn write_clipboard(&self, _text: &str) -> bool {
        false
    }
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
