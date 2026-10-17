//! `RuntimePort` onto the qrtz-lifecycle [`NoteRuntime`] (feature "runtime"): every recording channel of
//! docs/rust-core-api.md §4.3, the helper line/status hooks (Electron `index.ts` `helper.onStatus` + `onLine`),
//! and the record toggle. Deferred deletes are the runtime's tombstones (`schedule_delete` / `undo_delete`, N17);
//! the runtime emits `note:changed` for them itself.
use crate::ports::RuntimePort;
use async_trait::async_trait;
use qrtz_helper_bridge::{HelperState, HelperStatus};
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_lifecycle::runtime::{AddNoteRequest, CoreError, NoteRuntime};
use qrtz_model::{HelperHealth, OsPermission, StepId, StopReason};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What capture backend this run has (Electron `helperAvailability`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperAvailability {
    /// The Swift helper.
    Native,
    /// The mock helper (Linux dev, e2e).
    Simulated,
    /// macOS without the bundled helper (broken install), or no helper at all.
    Missing,
}

pub const HELPER_MISSING_DETAIL: &str =
    "The qrtz-helper binary is not in this build. Reinstall qrtz or build native/qrtz-helper.";

pub struct NoteRuntimePort {
    pub runtime: NoteRuntime,
    availability: HelperAvailability,
    was_running: AtomicBool,
    /// N123: `note:delete` (at once or scheduled) drops the meeting's cached part summaries here.
    summaries: Option<Arc<crate::summary::OnDeviceSummaries>>,
}

impl NoteRuntimePort {
    pub fn new(runtime: NoteRuntime, availability: HelperAvailability) -> Self {
        match availability {
            HelperAvailability::Missing => {
                runtime.set_helper_health(HelperHealth::Missing, Some(HELPER_MISSING_DETAIL.into()))
            }
            HelperAvailability::Simulated => runtime.set_helper_health(HelperHealth::Simulated, None),
            HelperAvailability::Native => {}
        }
        Self {
            runtime,
            availability,
            was_running: AtomicBool::new(false),
            summaries: None,
        }
    }

    /// N123: forget a deleted meeting's part summaries in this engine (the app's; `boot` returns it).
    pub fn forget_summaries_on_delete(mut self, e: Option<Arc<crate::summary::OnDeviceSummaries>>) -> Self {
        self.summaries = e;
        self
    }
}

/// A runtime error as the IPC error the renderer sees (unknown codes become E_INTERNAL).
pub fn app_error(e: CoreError) -> AppError {
    let code = ErrorCode::ALL
        .into_iter()
        .find(|c| c.as_str() == e.code)
        .unwrap_or(ErrorCode::Internal);
    AppError::new(code, e.message).retryable(e.retryable)
}

fn bad(m: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::Validation, m)
}

fn field<T: DeserializeOwned>(p: &Value, k: &str) -> Result<T, AppError> {
    serde_json::from_value(p[k].clone()).map_err(|e| bad(format!("{k}: {e}")))
}

fn opt_str(p: &Value, k: &str) -> Option<String> {
    p[k].as_str().map(str::to_owned)
}

fn to_json<T: serde::Serialize>(v: T) -> Result<Value, AppError> {
    serde_json::to_value(v).map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))
}

#[async_trait]
impl RuntimePort for NoteRuntimePort {
    async fn call(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        let rt = &self.runtime;
        match channel {
            "note:session" => to_json(rt.session()),
            "note:start" => to_json(rt.start(opt_str(&p, "title")).await.map_err(app_error)?),
            "note:stop" => to_json(rt.stop(StopReason::Manual).await.map_err(app_error)?),
            "note:dismiss" => to_json(rt.dismiss()),
            "note:pause" => to_json(rt.pause(p["paused"].as_bool()).map_err(app_error)?),
            // `{id, deferMs?}`: refuses the live meeting before any tombstone; 0/absent deletes now.
            "note:delete" => {
                let id: String = field(&p, "id")?;
                let defer = p["deferMs"].as_f64().unwrap_or(0.0).max(0.0) as u64;
                rt.schedule_delete(&id, defer).await.map_err(app_error)?;
                // N123: deleted or tombstoned, its part summaries go now (an undo just summarizes from the start).
                if let Some(e) = &self.summaries {
                    e.forget(&id);
                }
                Ok(json!({ "ok": true }))
            }
            "note:undo-delete" => {
                let id: String = field(&p, "id")?;
                Ok(json!({ "restored": rt.undo_delete(&id) }))
            }
            "note:pipeline" => {
                let id: String = field(&p, "id")?;
                to_json(rt.pipeline(&id).await.map_err(app_error)?)
            }
            "note:retry-step" => {
                let id: String = field(&p, "id")?;
                let step: StepId = field(&p, "step")?;
                to_json(rt.retry_step(&id, step).await.map_err(app_error)?)
            }
            "note:live-transcript" => {
                let id = opt_str(&p, "meetingId");
                to_json(rt.live_transcript(id.as_deref()))
            }
            "note:resummarize" => {
                let id: String = field(&p, "id")?;
                to_json(rt.resummarize(&id).await.map_err(app_error)?)
            }
            "note:set-template" => {
                let id: String = field(&p, "id")?;
                let t: String = field(&p, "template")?;
                to_json(rt.set_template(&id, &t).await.map_err(app_error)?)
            }
            "note:add-note" => {
                let req: AddNoteRequest = serde_json::from_value(p).map_err(|e| bad(e.to_string()))?;
                to_json(rt.add_note(req).await.map_err(app_error)?)
            }
            "permissions:get" => to_json(rt.permissions()),
            "permissions:request" => {
                let perm: OsPermission = field(&p, "permission")?;
                to_json(rt.request_permission(perm).await.map_err(app_error)?)
            }
            _ => Err(qrtz_ipc::no_handler(channel)),
        }
    }
    async fn toggle(&self) -> Result<Value, AppError> {
        to_json(self.runtime.toggle().await.map_err(app_error)?)
    }
    fn helper_line(&self, msg: &Value) {
        self.runtime.helper_json(msg)
    }
    /// Electron `helper.onStatus`: health follows the supervisor; a running helper that goes away is F3
    /// (`helper_down`); an open circuit is F4 (`helper_failed`).
    fn helper_status(&self, s: &HelperStatus) {
        let reason = || {
            s.last_error.clone().unwrap_or_else(|| {
                serde_json::to_value(s.state)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            })
        };
        if self.availability != HelperAvailability::Missing {
            match s.state {
                HelperState::Running => self.runtime.set_helper_health(
                    if self.availability == HelperAvailability::Simulated {
                        HelperHealth::Simulated
                    } else {
                        HelperHealth::Ok
                    },
                    None,
                ),
                HelperState::Starting => {}
                _ => self.runtime.set_helper_health(HelperHealth::Down, Some(reason())),
            }
        }
        if s.state == HelperState::Running {
            self.was_running.store(true, Ordering::SeqCst);
        } else if s.state != HelperState::Starting && self.was_running.swap(false, Ordering::SeqCst) {
            self.runtime.helper_down(&reason());
        }
        if s.state == HelperState::Failed {
            self.runtime.helper_failed(&reason());
        }
    }
}
