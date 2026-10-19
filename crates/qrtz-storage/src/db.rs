use std::collections::HashMap;
use std::path::{Path, PathBuf};

use qrtz_model::{Meeting, SegmentSource};
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
        // Same pragmas as apps/desktop/src/worker/db.ts. Connection-only pragmas first, then refuse a database from
        // a newer app before anything writes to the file: switching to WAL rewrites the header (N19).
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        let (db, app) = (
            crate::migrate::schema_version(&conn)?,
            MIGRATIONS.iter().map(|m| m.version).max().unwrap_or(0),
        );
        if db > app {
            return Err(StorageError::SchemaTooNew { db, app });
        }
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        let report = migrate(&mut conn, MIGRATIONS)?;
        Ok(Db {
            conn,
            migration: Some(report),
        })
    }

    /// Opens (creating if needed) and migrates the app database: WAL, foreign keys on, busy_timeout 5000. The
    /// file and its SQLite companions are owner-only (0600) whatever the umask (N25, `ownerOnlyDbFiles`).
    pub fn open(path: impl AsRef<Path>) -> Result<Db> {
        owner_only_db_files(path.as_ref());
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

    /// nano's Apple Calendar cache (ADR 0216).
    pub fn calendar(&self) -> crate::CalendarRepo<'_> {
        crate::CalendarRepo::new(&self.conn)
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
        // The snapshot is the whole library: a fresh 0600 file, created exclusively so a symlink planted at the
        // path is removed, never followed (N26, `freshOwnerOnlyFile`). `VACUUM INTO` accepts an empty file.
        if path.symlink_metadata().is_ok() {
            std::fs::remove_file(path)?;
        }
        create_owner_only(path, true)?;
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
        struct Map<'a>(&'a HashMap<String, PathBuf>);
        impl ImportHooks for Map<'_> {
            fn audio(&mut self, _m: &Meeting, base: &str) -> Result<Option<String>> {
                Ok(self.0.get(base).map(|p| p.to_string_lossy().into_owned()))
            }
        }
        self.import_from_with(src, &mut Map(audio_map))
    }

    /// [`Db::import_from`] with caller hooks (qrtz-backup): `skip` adds skip reasons on top of "id already
    /// here", `audio` places a meeting's recording just before its transaction, and `rolled_back` undoes that
    /// placement when the meeting's transaction fails, so a meeting lands whole (row, transcript, audio) or not
    /// at all.
    pub fn import_from_with(
        &self,
        src: impl AsRef<Path>,
        hooks: &mut dyn ImportHooks,
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
            if to.get_meeting(&m.id)?.is_some() || hooks.skip(&m.id) {
                skipped += 1;
                continue;
            }
            let base = m
                .audio_path
                .as_ref()
                .and_then(|p| Some(Path::new(p).file_name()?.to_string_lossy().into_owned()));
            let audio = match &base {
                Some(b) => hooks.audio(&m, b)?,
                None => None,
            };
            let mut row = m.clone();
            row.audio_path = audio;
            // One meeting is one transaction: a failure mid-meeting must not leave a meeting row without its
            // transcript, because a re-import skips ids already present and would never repair it (N11).
            let r = to.tx(|| {
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
            });
            if let Err(e) = r {
                hooks.rolled_back(&m);
                return Err(e);
            }
            imported += 1;
        }
        Ok(ImportReport { imported, skipped })
    }
}

/// Hooks for [`Db::import_from_with`].
pub trait ImportHooks {
    /// `true`: skip this meeting (counted as skipped) even though its id isn't in the database.
    fn skip(&mut self, _id: &str) -> bool {
        false
    }
    /// The meeting's recording (backup file name `base`): return the path to store, or None to drop it.
    fn audio(&mut self, m: &Meeting, base: &str) -> Result<Option<String>>;
    /// The meeting's transaction failed after `audio` ran: undo whatever `audio` placed.
    fn rolled_back(&mut self, _m: &Meeting) {}
}

/// SQLite's companion files: the unix VFS creates them with the main file's mode.
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// Creates `path` with mode 0600 (`exclusive`: fail if it exists, never follow a symlink). Elsewhere than unix, a
/// plain create.
fn create_owner_only(path: &Path, exclusive: bool) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true);
    if exclusive {
        o.create_new(true);
    } else {
        o.create(true).append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path).map(drop)
}

/// N25 (`ownerOnlyDbFiles` in `apps/desktop/src/worker/db.ts`): the DB holds every transcript, so it is created
/// 0600 before SQLite opens it (`-wal`/`-shm`/`-journal` then follow, SQLite copies the main file's mode), and
/// files an older build left at the umask default are tightened. Best effort: the open itself still decides.
fn owner_only_db_files(path: &Path) {
    let s = path.to_string_lossy();
    if s.is_empty() || s == ":memory:" || s.starts_with("file:") {
        return;
    }
    if create_owner_only(path, false).is_err() {
        return; // missing directory, read-only volume: let SQLite report it
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let all = std::iter::once(path.to_path_buf())
            .chain(SIDECARS.iter().map(|x| PathBuf::from(format!("{s}{x}"))));
        for f in all {
            let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600));
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
