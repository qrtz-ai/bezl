//! `RuntimePort` onto the qrtz-lifecycle [`NoteRuntime`] (feature "runtime"). Only the channels wired so far are
//! served by the runtime; every other channel and the helper hooks go to `rest` (the stub until the Tauri
//! migration resumes). `pending_delete` stays `None` until the undo flow lands in the shell.
use crate::ports::RuntimePort;
use async_trait::async_trait;
use qrtz_helper_bridge::HelperStatus;
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_lifecycle::runtime::{CoreError, NoteRuntime};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct NoteRuntimePort {
    pub runtime: NoteRuntime,
    pub rest: Arc<dyn RuntimePort>,
}

/// A runtime error as the IPC error the renderer sees (unknown codes become E_INTERNAL).
pub fn app_error(e: CoreError) -> AppError {
    let code = ErrorCode::ALL
        .into_iter()
        .find(|c| c.as_str() == e.code)
        .unwrap_or(ErrorCode::Internal);
    AppError::new(code, e.message).retryable(e.retryable)
}

#[async_trait]
impl RuntimePort for NoteRuntimePort {
    async fn call(&self, channel: &str, payload: Value) -> Result<Value, AppError> {
        match channel {
            // `{id, deferMs?}`: no tombstones in this shell yet, so a deferred delete runs now. The service emits
            // note:changed after it; the response keeps the contract's `{ok: true}`.
            "note:delete" => {
                let id = payload["id"].as_str().unwrap_or_default();
                self.runtime.delete_meeting(id).await.map_err(app_error)?;
                Ok(json!({ "ok": true }))
            }
            _ => self.rest.call(channel, payload).await,
        }
    }
    async fn toggle(&self) -> Result<Value, AppError> {
        self.rest.toggle().await
    }
    fn helper_line(&self, msg: &Value) {
        self.rest.helper_line(msg)
    }
    fn helper_status(&self, status: &HelperStatus) {
        self.rest.helper_status(status)
    }
}
