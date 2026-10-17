//! `StoragePort` over the qrtz-storage `StorageHandle` (feature "runtime"): the storage rows of
//! docs/rust-core-api.md §4.3 (Electron `index.ts` → `worker.call(...)`).
use crate::markdown::{export_filename, meeting_to_markdown};
use crate::ports::StoragePort;
use crate::service::Events;
use async_trait::async_trait;
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_storage::{Db, ListQuery, StorageError, StorageHandle};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Storage error → IPC error (same codes the worker reported).
pub fn storage_error(e: StorageError) -> AppError {
    let code = ErrorCode::ALL
        .into_iter()
        .find(|c| c.as_str() == e.code())
        .unwrap_or(ErrorCode::Internal);
    AppError::new(code, e.to_string()).retryable(e.retryable())
}

/// Shell-side effects of storage channels (clipboard, save dialog). The Tauri shell implements them.
pub trait StorageShell: Send + Sync {
    /// `note:export {save: false}`: main-process clipboard (works while unfocused).
    fn write_clipboard(&self, _text: &str) {}
    /// `note:export {save: true}`: where to save (`None` = cancelled). Default: `QRTZ_EXPORT_DIR` or cancel.
    fn save_path(&self, filename: &str) -> Option<PathBuf> {
        std::env::var_os("QRTZ_EXPORT_DIR").map(|d| PathBuf::from(d).join(filename))
    }
}
pub struct NoStorageShell;
impl StorageShell for NoStorageShell {}

/// Meetings whose delete is pending (undo window): hidden from `note:list`. Bozniak's tombstones plug in here.
pub type Hidden = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

pub struct SqliteStorage {
    pub db: StorageHandle,
    pub events: Events,
    /// The live meeting (`note:list-notes` without an id resolves to it).
    pub live_meeting: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub hidden: Option<Hidden>,
    pub shell: Arc<dyn StorageShell>,
}

fn bad(m: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::Validation, m)
}
fn need_str(p: &Value, k: &str) -> Result<String, AppError> {
    p[k].as_str()
        .map(str::to_owned)
        .ok_or_else(|| bad(format!("{k} is required")))
}

impl SqliteStorage {
    async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> qrtz_storage::Result<R> + Send + 'static,
    ) -> Result<R, AppError> {
        self.db.call(f).await.map_err(storage_error)
    }
    fn json<T: serde::Serialize>(v: T) -> Result<Value, AppError> {
        serde_json::to_value(v).map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))
    }
}

#[async_trait]
impl StoragePort for SqliteStorage {
    async fn call(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        match channel {
            "note:list" => {
                let q: ListQuery = serde_json::from_value(if p.is_null() { json!({}) } else { p })
                    .map_err(|e| bad(e.to_string()))?;
                let mut rows = self.run(move |db| db.notes().list(&q)).await?;
                if let Some(h) = &self.hidden {
                    let gone = h();
                    if !gone.is_empty() {
                        rows.retain(|r| !gone.contains(&r.meeting.id));
                    }
                }
                Self::json(rows)
            }
            "note:get" => {
                let id = need_str(&p, "id")?;
                Self::json(self.run(move |db| db.notes().detail(&id)).await?)
            }
            "note:list-notes" => {
                let id = p["meetingId"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| (self.live_meeting)());
                match id {
                    Some(id) => Self::json(self.run(move |db| db.notes().get_notes(&id)).await?),
                    None => Ok(json!([])),
                }
            }
            "note:update-note" => {
                let (id, text) = (need_str(&p, "id")?, need_str(&p, "text")?);
                let ok = self.run(move |db| db.notes().update_note(&id, &text)).await?;
                Ok(json!({ "ok": ok }))
            }
            "note:delete-note" => {
                let id = need_str(&p, "id")?;
                let ok = self.run(move |db| db.notes().delete_note(&id)).await?;
                Ok(json!({ "ok": ok }))
            }
            "note:rename-speaker" => {
                let (mid, from, to) = (
                    need_str(&p, "meetingId")?,
                    need_str(&p, "from")?,
                    need_str(&p, "to")?,
                );
                let m2 = mid.clone();
                let changed = self
                    .run(move |db| {
                        db.notes().need(&m2)?;
                        db.notes().rename_speaker(&m2, &from, &to)
                    })
                    .await?;
                self.events.emit("note:changed", json!({ "id": mid }));
                Ok(json!({ "changed": changed }))
            }
            "note:export" => {
                let id = need_str(&p, "id")?;
                let detail = Self::json(self.run(move |db| db.notes().detail(&id)).await?)?;
                let m = &detail["meeting"];
                let filename = export_filename(
                    m["title"].as_str().unwrap_or(""),
                    m["startedAt"].as_i64().unwrap_or(0),
                );
                let markdown = meeting_to_markdown(&detail);
                if p["save"].as_bool() != Some(true) {
                    self.shell.write_clipboard(&markdown);
                    return Ok(json!({ "filename": filename, "markdown": markdown }));
                }
                let mut out = json!({ "filename": filename, "markdown": markdown });
                if let Some(target) = self.shell.save_path(&filename) {
                    std::fs::write(&target, &markdown)
                        .map_err(|e| AppError::new(ErrorCode::Internal, format!("export failed: {e}")))?;
                    out["savedTo"] = json!(target.to_string_lossy());
                }
                Ok(out)
            }
            "db:health" => Self::json(self.run(|db| db.health()).await?),
            _ => Err(AppError::new(
                ErrorCode::Unavailable,
                format!("{channel} is not available in the Tauri build yet"),
            )
            .retryable(false)),
        }
    }
}
