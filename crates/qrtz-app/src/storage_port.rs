//! `StoragePort` over the qrtz-storage `StorageHandle` (feature "runtime"): the storage rows of
//! docs/rust-core-api.md §4.3 (Electron `index.ts` → `worker.call(...)`).
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
    /// `data:export-backup` when `QRTZ_BACKUP_PATH` is unset: a save dialog (zip) with this default name in
    /// Documents. `None` = cancelled (the default: no dialog in headless builds).
    fn backup_save_path(&self, _default_name: &str) -> Option<PathBuf> {
        None
    }
    /// `data:import-backup` when `QRTZ_BACKUP_PATH` is unset: an open dialog (zip). `None` = cancelled.
    fn backup_open_path(&self) -> Option<PathBuf> {
        None
    }
}

/// `qrtz-backup-<UTC ISO[0..16] with ':' and 'T' → '-'>.zip`, e.g. `qrtz-backup-2026-10-17-14-32.zip` (backup.ts).
pub fn backup_filename(now_ms: i64) -> String {
    let secs = now_ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days (Howard Hinnant), proleptic Gregorian, UTC.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "qrtz-backup-{y:04}-{m:02}-{d:02}-{:02}-{:02}.zip",
        rem / 3600,
        rem % 3600 / 60
    )
}

fn backup_env() -> Option<PathBuf> {
    std::env::var_os("QRTZ_BACKUP_PATH")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `data:backup-progress` at most about 10 per second (the final `done == total` always goes out).
fn throttled_progress(events: Events, kind: &'static str) -> impl FnMut(qrtz_backup::Phase, u64, u64) + Send {
    let mut last: Option<(std::time::Instant, &'static str)> = None;
    move |phase, done, total| {
        let phase = match phase {
            qrtz_backup::Phase::Write => "write",
            qrtz_backup::Phase::Verify => "verify",
            qrtz_backup::Phase::Import => "import",
        };
        let due = match last {
            Some((t, p)) => {
                p != phase || done >= total || t.elapsed() >= std::time::Duration::from_millis(100)
            }
            None => true,
        };
        if due {
            last = Some((std::time::Instant::now(), phase));
            events.emit(
                "data:backup-progress",
                json!({"kind": kind, "phase": phase, "done": done, "total": total}),
            );
        }
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
    /// `<data>/recordings` (backup export reads it, import restores into it).
    pub recordings_dir: PathBuf,
    /// The same tombstone store as `RuntimeDeps::pending_delete`: import skips meetings pending delete.
    pub pending: Option<Arc<qrtz_storage::PendingDeletes>>,
    /// Written into the backup manifest.
    pub app_version: String,
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
                let e = self.run(move |db| qrtz_backup::export_meeting(db, &id)).await?;
                let mut out = json!({ "filename": e.filename, "markdown": e.markdown });
                if p["save"].as_bool() != Some(true) {
                    self.shell.write_clipboard(&e.markdown);
                    return Ok(out);
                }
                let shell = self.shell.clone();
                let name = e.filename.clone();
                let target = tokio::task::spawn_blocking(move || shell.save_path(&name))
                    .await
                    .unwrap_or(None);
                if let Some(target) = target {
                    std::fs::write(&target, &e.markdown)
                        .map_err(|e| AppError::new(ErrorCode::Internal, format!("export failed: {e}")))?;
                    out["savedTo"] = json!(target.to_string_lossy());
                }
                Ok(out)
            }
            "data:export-backup" => {
                let target = match backup_env() {
                    Some(t) => Some(t),
                    None => {
                        let (shell, name) = (self.shell.clone(), backup_filename(now_ms()));
                        tokio::task::spawn_blocking(move || shell.backup_save_path(&name))
                            .await
                            .unwrap_or(None)
                    }
                };
                let Some(target) = target else {
                    return Ok(json!({"meetings": 0, "audioFiles": 0, "bytes": 0}));
                };
                let (rec, ver, ev) = (
                    self.recordings_dir.clone(),
                    self.app_version.clone(),
                    self.events.clone(),
                );
                let report = self
                    .run(move |db| {
                        let mut progress = throttled_progress(ev, "export");
                        qrtz_backup::export_backup(
                            &qrtz_backup::ExportBackup {
                                db,
                                target: &target,
                                recordings_dir: &rec,
                                temp_dir: &std::env::temp_dir(),
                                app_version: &ver,
                                now_ms: now_ms(),
                            },
                            &mut progress,
                        )
                    })
                    .await?;
                self.events.emit(
                    "data:backup-progress",
                    json!({"kind": "export", "phase": "done", "done": 1, "total": 1}),
                );
                Self::json(report)
            }
            "data:import-backup" => {
                let source = match backup_env() {
                    Some(s) => Some(s),
                    None => {
                        let shell = self.shell.clone();
                        tokio::task::spawn_blocking(move || shell.backup_open_path())
                            .await
                            .unwrap_or(None)
                    }
                };
                let Some(source) = source else {
                    return Ok(json!({"canceled": true, "imported": 0, "skipped": 0, "audioFiles": 0}));
                };
                let (rec, pending, ev) = (
                    self.recordings_dir.clone(),
                    self.pending.clone(),
                    self.events.clone(),
                );
                let report = self
                    .run(move |db| {
                        let mut progress = throttled_progress(ev, "import");
                        qrtz_backup::import_backup(
                            &qrtz_backup::ImportBackup {
                                db,
                                source: &source,
                                recordings_dir: &rec,
                                temp_dir: &std::env::temp_dir(),
                                pending: pending.as_deref(),
                                limits: qrtz_backup::ImportLimits::default(),
                            },
                            &mut progress,
                        )
                    })
                    .await?;
                self.events.emit(
                    "data:backup-progress",
                    json!({"kind": "import", "phase": "done", "done": 1, "total": 1}),
                );
                self.events.emit("note:changed", json!({ "id": "*" }));
                Self::json(report)
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
