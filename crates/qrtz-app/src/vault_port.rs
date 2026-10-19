//! ADR 0220 phase 1: nano's notes vault on the host (`vault:*`, `doc:*`, the `doc:changed` event).
//!
//! Confinement: only this host module opens the vault (a folder the user can see) and its index (in the app's
//! data dir). No helper gets the path or a read. The folder is qrtz's suggestion (`~/Documents/qrtz Notes`) or
//! one the user picked in the host's folder panel (`vault:choose`); the renderer never names a path, and every
//! file path is resolved inside the vault by `qrtz_storage::vault` (no `..`, no symlink escapes).
//!
//! First open migrates the old per-meeting notes (each meeting with notes becomes one `.md` note linked to it),
//! after a JSON backup of every old note in `<data>/backups/`. The old rows stay as they are.
//! A poll watcher (stat only) reindexes changed files every [`POLL`] and emits `doc:changed`; FSEvents is deferred.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qrtz_ipc::{AppError, ErrorCode};
use qrtz_storage::vault::{migrate_meeting_notes, LegacyMeeting, Vault, VaultError};
use qrtz_storage::StorageHandle;
use serde_json::{json, Value};

use crate::service::Events;
use crate::storage_port::StorageShell;

pub const PREFIXES: [&str; 2] = ["doc:", "vault:"];
pub const POLL: Duration = Duration::from_secs(2);
const SETTINGS_FILE: &str = "vault.json";
const BACKUP_FILE: &str = "notes-before-vault.json";

pub struct NotesVault {
    data: PathBuf,
    db: StorageHandle,
    events: Events,
    shell: Arc<dyn StorageShell>,
    open: Mutex<Option<Vault>>,
    /// One first open (and migration) at a time.
    opening: tokio::sync::Mutex<()>,
}

fn verr(e: VaultError) -> AppError {
    match e {
        VaultError::Conflict { current_hash } => {
            AppError::new(ErrorCode::Validation, "the note changed outside qrtz")
                .details(json!({"conflict": true, "currentHash": current_hash}))
        }
        VaultError::NotFound(id) => AppError::new(ErrorCode::NotFound, format!("note {id} not found")),
        VaultError::Escape => AppError::new(ErrorCode::Forbidden, "path is outside the notes folder"),
        VaultError::Invalid(m) => AppError::new(ErrorCode::Validation, m),
        e => AppError::new(ErrorCode::Internal, e.to_string()).retryable(true),
    }
}
fn bad(m: &str) -> AppError {
    AppError::new(ErrorCode::Validation, m.to_owned())
}
fn s<'a>(p: &'a Value, k: &str) -> Result<&'a str, AppError> {
    p[k].as_str().ok_or_else(|| bad(&format!("{k} is required")))
}

fn to<T: serde::Serialize>(d: T) -> Result<Value, AppError> {
    serde_json::to_value(d).map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))
}

impl NotesVault {
    pub fn new(data: PathBuf, db: StorageHandle, events: Events, shell: Arc<dyn StorageShell>) -> Arc<Self> {
        Arc::new(NotesVault {
            data,
            db,
            events,
            shell,
            open: Mutex::new(None),
            opening: tokio::sync::Mutex::new(()),
        })
    }

    fn default_root(&self) -> PathBuf {
        self.shell
            .default_notes_dir()
            .unwrap_or_else(|| self.data.join("notes"))
    }
    fn chosen_root(&self) -> Option<PathBuf> {
        let t = std::fs::read_to_string(self.data.join(SETTINGS_FILE)).ok()?;
        let v: Value = serde_json::from_str(&t).ok()?;
        let p = PathBuf::from(v["path"].as_str()?);
        p.is_absolute().then_some(p)
    }
    fn root(&self) -> PathBuf {
        self.chosen_root().unwrap_or_else(|| self.default_root())
    }
    fn index_path(&self) -> PathBuf {
        self.data.join("vault-index").join("index.db")
    }

    /// The old notes, grouped per meeting (all meetings, newest first, paged).
    async fn legacy(&self) -> Result<Vec<LegacyMeeting>, AppError> {
        self.db
            .call(|db| {
                let mut out = vec![];
                let mut off = 0;
                loop {
                    let page = db.notes().list_meetings(500, off)?;
                    if page.is_empty() {
                        break;
                    }
                    off += page.len() as u32;
                    for m in page {
                        let notes = db.notes().get_notes(&m.id)?;
                        if !notes.is_empty() {
                            out.push(LegacyMeeting {
                                id: m.id.clone(),
                                title: m.title.clone(),
                                lines: notes.into_iter().map(|n| (n.at_ms, n.text)).collect(),
                            });
                        }
                    }
                }
                Ok(out)
            })
            .await
            .map_err(crate::storage_port::storage_error)
    }

    fn backup(&self, legacy: &[LegacyMeeting]) -> std::io::Result<()> {
        let dir = self.data.join("backups");
        crate::dirs::private_dir(&dir)?;
        let path = dir.join(BACKUP_FILE);
        if path.exists() {
            return Ok(());
        }
        let v: Vec<Value> = legacy
            .iter()
            .map(|m| {
                json!({"meetingId": m.id, "title": m.title,
                "notes": m.lines.iter().map(|(a, t)| json!({"atMs": a, "text": t})).collect::<Vec<_>>()})
            })
            .collect();
        qrtz_storage::vault::atomic_write(&path, serde_json::to_string_pretty(&v)?.as_bytes())
    }

    /// Open the vault (first call: create the folder, rebuild/refresh the index, migrate old notes).
    async fn ensure(&self) -> Result<(), AppError> {
        if self.open.lock().unwrap().is_some() {
            return Ok(());
        }
        let _one = self.opening.lock().await;
        if self.open.lock().unwrap().is_some() {
            return Ok(());
        }
        let legacy = self.legacy().await?;
        if !legacy.is_empty() {
            // Never migrate without a backup.
            self.backup(&legacy)
                .map_err(|e| AppError::new(ErrorCode::Internal, format!("notes backup failed: {e}")))?;
        }
        let (root, index) = (self.root(), self.index_path());
        let v = tokio::task::spawn_blocking(move || -> Result<Vault, VaultError> {
            let mut v = Vault::open(&root, &index)?;
            let done = migrate_meeting_notes(&mut v, &legacy)?;
            if !done.is_empty() {
                tracing::info!(meetings = done.len(), "vault: migrated meeting notes");
            }
            Ok(v)
        })
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
        .map_err(verr)?;
        let mut g = self.open.lock().unwrap();
        if g.is_none() {
            *g = Some(v);
        }
        Ok(())
    }

    async fn with<R: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&mut Vault) -> Result<R, VaultError> + Send + 'static,
    ) -> Result<R, AppError> {
        self.ensure().await?;
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut g = me.open.lock().unwrap();
            let v = g
                .as_mut()
                .ok_or_else(|| VaultError::Invalid("notes folder unavailable".into()))?;
            f(v)
        })
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
        .map_err(verr)
    }

    fn info(&self, v: &Vault) -> Value {
        let def = std::fs::canonicalize(self.default_root()).unwrap_or_else(|_| self.default_root());
        json!({"path": v.root().to_string_lossy(), "isDefault": v.root() == def,
               "count": v.count().unwrap_or(0)})
    }

    fn changed(&self, ids: Vec<String>) {
        if !ids.is_empty() {
            self.events.emit(
                "doc:changed",
                json!({"ids": ids.into_iter().take(1000).collect::<Vec<_>>()}),
            );
        }
    }

    pub async fn call(self: &Arc<Self>, channel: &str, p: Value) -> Result<Value, AppError> {
        match channel {
            "vault:get" => {
                self.ensure().await?;
                let g = self.open.lock().unwrap();
                Ok(self.info(g.as_ref().expect("opened")))
            }
            "vault:choose" => {
                let sh = self.shell.clone();
                let picked = tokio::task::spawn_blocking(move || sh.pick_notes_folder())
                    .await
                    .ok()
                    .flatten();
                let Some(path) = picked else {
                    self.ensure().await?;
                    let g = self.open.lock().unwrap();
                    let mut v = self.info(g.as_ref().expect("opened"));
                    v["canceled"] = json!(true);
                    return Ok(v);
                };
                self.switch_to(&path).await?;
                let g = self.open.lock().unwrap();
                let mut v = self.info(g.as_ref().expect("opened"));
                v["canceled"] = json!(false);
                Ok(v)
            }
            "doc:list" => {
                let q = p["query"].as_str().map(str::to_owned);
                let m = p["meetingId"].as_str().map(str::to_owned);
                let lim = p["limit"].as_u64().unwrap_or(2000) as u32;
                to(self
                    .with(move |v| v.list(q.as_deref(), m.as_deref(), lim))
                    .await?)
            }
            "doc:get" => {
                let id = s(&p, "id")?.to_owned();
                to(self.with(move |v| v.get(&id)).await?)
            }
            "doc:create" => {
                let t = p["title"].as_str().map(str::to_owned);
                let m = p["meetingId"].as_str().map(str::to_owned);
                let d = self
                    .with(move |v| v.create(t.as_deref(), m.as_deref(), None))
                    .await?;
                self.changed(vec![d.id.clone()]);
                to(d)
            }
            "doc:save" => {
                let (id, body, base) = (
                    s(&p, "id")?.to_owned(),
                    s(&p, "body")?.to_owned(),
                    s(&p, "baseHash")?.to_owned(),
                );
                let force = p["force"].as_bool().unwrap_or(false);
                let d = self.with(move |v| v.save(&id, &body, &base, force)).await?;
                self.changed(vec![d.id.clone()]);
                to(d)
            }
            "doc:trash" => {
                let id = s(&p, "id")?.to_owned();
                let i2 = id.clone();
                self.with(move |v| v.trash(&i2)).await?;
                self.changed(vec![id]);
                Ok(json!({"ok": true}))
            }
            "doc:link-meeting" | "doc:unlink-meeting" => {
                let (id, m) = (s(&p, "id")?.to_owned(), s(&p, "meetingId")?.to_owned());
                let link = channel == "doc:link-meeting";
                let d = self
                    .with(move |v| {
                        if link {
                            v.link_meeting(&id, &m)
                        } else {
                            v.unlink_meeting(&id, &m)
                        }
                    })
                    .await?;
                self.changed(vec![d.id.clone()]);
                to(d)
            }
            "doc:rebuild-index" => {
                let n = self.with(|v| v.rebuild()).await?;
                self.changed(vec![]);
                Ok(json!({"count": n}))
            }
            _ => Err(qrtz_ipc::no_handler(channel)),
        }
    }

    /// Use another folder: open it (migrated notes stay where they were written; the marker in each file keeps a
    /// later migration from repeating), then remember it.
    pub async fn switch_to(self: &Arc<Self>, path: &Path) -> Result<(), AppError> {
        if !path.is_absolute() {
            return Err(bad("the notes folder must be an absolute path"));
        }
        let (root, index) = (path.to_path_buf(), self.index_path());
        let v = tokio::task::spawn_blocking(move || {
            // A new folder gets a fresh index.
            let _ = std::fs::remove_file(&index);
            Vault::open(&root, &index)
        })
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
        .map_err(verr)?;
        let canon = v.root().to_string_lossy().into_owned();
        crate::dirs::private_dir(&self.data).ok();
        qrtz_storage::vault::atomic_write(
            &self.data.join(SETTINGS_FILE),
            json!({"path": canon}).to_string().as_bytes(),
        )
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?;
        *self.open.lock().unwrap() = Some(v);
        self.changed(vec![]);
        Ok(())
    }

    /// The mini HUD's quick add (after `note:add-note` stored the line): append it to the meeting's note.
    pub async fn hud_line(self: &Arc<Self>, meeting_id: String, text: String) -> Result<(), AppError> {
        let mid = meeting_id.clone();
        let title = self
            .db
            .call(move |db| Ok(db.notes().get_meeting(&mid)?.map(|m| m.title)))
            .await
            .map_err(crate::storage_port::storage_error)?
            .unwrap_or_else(|| "meeting notes".to_owned());
        let d = self
            .with(move |v| v.append_to_meeting(&meeting_id, &title, &text))
            .await?;
        self.changed(vec![d.id]);
        Ok(())
    }

    /// One poll: reindex what changed on disk (only once the vault was opened).
    pub fn poll(&self) {
        let rep = {
            let mut g = self.open.lock().unwrap();
            match g.as_mut() {
                Some(v) => v.rescan(),
                None => return,
            }
        };
        match rep {
            Ok(r) if !r.is_empty() => self.changed(r.ids()),
            Ok(_) => {}
            Err(e) => tracing::warn!(kind = e.kind(), "vault rescan failed"),
        }
    }

    pub fn spawn_watcher(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(POLL).await;
                let Some(v) = me.upgrade() else { break };
                let _ = tokio::task::spawn_blocking(move || v.poll()).await;
            }
        });
    }
}
