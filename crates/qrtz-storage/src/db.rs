use std::collections::HashMap;
use std::path::{Path, PathBuf};

use qrtz_model::SegmentSource;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::lifecycle::LifecycleRepo;
use crate::migrate::{migrate, schema_version, MigrationReport, MIGRATIONS};
use crate::notes::NoteRepo;
use crate::{Result, StorageError};

/// `db:health` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbHealth {
    pub ok: bool,
    pub schema_version: u32,
    pub fts5: bool,
}

/// `data.importDb` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub imported: u32,
    pub skipped: u32,
}

/// One SQLite connection with the app schema. Not `Sync`: share it through [`crate::StorageHandle`].
pub struct Db {
    conn: Connection,
    migration: Option<MigrationReport>,
}

impl Db {
    fn configure(mut conn: Connection) -> Result<Db> {
        // Same pragmas as apps/desktop/src/worker/db.ts.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        let report = migrate(&mut conn, MIGRATIONS)?;
        Ok(Db {
            conn,
            migration: Some(report),
        })
    }

    /// Opens (creating if needed) and migrates the app database: WAL, foreign keys on, busy_timeout 5000.
    pub fn open(path: impl AsRef<Path>) -> Result<Db> {
        Db::configure(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Db> {
        Db::configure(Connection::open_in_memory()?)
    }

    /// Read-only, no migrations (backup import source). Fails unless `PRAGMA integrity_check` is ok.
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Db> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if ok != "ok" {
            return Err(StorageError::Validation(format!(
                "backup database is corrupt: {ok}"
            )));
        }
        Ok(Db {
            conn,
            migration: None,
        })
    }

    /// What `open` migrated (None for read-only handles).
    pub fn migration(&self) -> Option<&MigrationReport> {
        self.migration.as_ref()
    }

    pub fn schema_version(&self) -> Result<u32> {
        schema_version(&self.conn)
    }

    /// `db:health`.
    pub fn health(&self) -> Result<DbHealth> {
        let fts5 = self
            .conn
            .execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS temp._fts_probe USING fts5(x); DROP TABLE temp._fts_probe;")
            .is_ok();
        Ok(DbHealth {
            ok: true,
            schema_version: self.schema_version()?,
            fts5,
        })
    }

    pub fn notes(&self) -> NoteRepo<'_> {
        NoteRepo::new(&self.conn)
    }

    pub fn lifecycle(&self) -> LifecycleRepo<'_> {
        LifecycleRepo::new(&self.conn)
    }

    /// Escape hatch for tests and one-off reads. Never use it to run a plain `VACUUM` (see crate docs).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// `data.snapshot`: a consistent online copy via `VACUUM INTO` (does not renumber the live DB's rowids).
    pub fn snapshot_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        self.conn.execute("VACUUM INTO ?", [path.to_string_lossy()])?;
        Ok(())
    }

    /// `data.importDb`: merges meetings from a backup DB that aren't present here (segments, summary, notes).
    /// `audio_map` maps an audio file name in the backup to its restored path.
    pub fn import_from(
        &self,
        src: impl AsRef<Path>,
        audio_map: &HashMap<String, PathBuf>,
    ) -> Result<ImportReport> {
        let src = Db::open_readonly(src)?;
        let v = src.schema_version()?;
        if v > self.schema_version()? {
            return Err(StorageError::Validation(format!(
                "backup is from a newer qrtz (schema {v})"
            )));
        }
        let (from, to) = (src.notes(), self.notes());
        let has_notes = v >= 4;
        let (mut imported, mut skipped) = (0, 0);
        for m in from.list_meetings(1_000_000, 0)? {
            if to.get_meeting(&m.id)?.is_some() {
                skipped += 1;
                continue;
            }
            let audio = m.audio_path.as_ref().and_then(|p| {
                let base = Path::new(p).file_name()?.to_string_lossy().into_owned();
                audio_map.get(&base).map(|p| p.to_string_lossy().into_owned())
            });
            let mut row = m.clone();
            row.audio_path = audio;
            // One meeting is one transaction: a failure mid-meeting must not leave a meeting row without its
            // transcript, because a re-import skips ids already present and would never repair it (N11).
            to.tx(|| {
                to.upsert_meeting(&row)?;
                if let Some(t) = &m.template {
                    to.set_template(&m.id, t)?;
                }
                let segs = from.get_segments(&m.id)?;
                for kind in [SegmentSource::Live, SegmentSource::Post] {
                    let part: Vec<_> = segs.iter().filter(|s| s.source == kind).cloned().collect();
                    if !part.is_empty() {
                        to.replace_segments(&m.id, kind, &part)?;
                    }
                }
                if let Some(s) = from.get_summary(&m.id)? {
                    to.save_summary(&m.id, &s, now_ms())?;
                }
                if has_notes {
                    for n in from.get_notes(&m.id)? {
                        to.add_note(&m.id, &n)?;
                    }
                }
                Ok(())
            })?;
            imported += 1;
        }
        Ok(ImportReport { imported, skipped })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
