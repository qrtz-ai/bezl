//! Deferred (undoable) meeting deletes, made durable with a tombstone file: the port of the Electron main
//! process's `PendingDeletes` (`apps/desktop/src/main/pending-deletes.ts`), file format included, so a
//! tombstone left by one build is completed by the other.
//!
//! A tombstone is written the moment the user confirms; the real delete runs when the undo window ends (the
//! timer lives in `qrtz-lifecycle`'s `NoteRuntime`). If the app quits or crashes first, the tombstone is completed
//! on the next launch. Lists hide tombstoned meetings in the meantime, and startup recovery never resumes their
//! post-processing (N17, ADR 0016).
//!
//! The file is `<dir>/pending-deletes.json`, a JSON array of meeting ids (`["a","b"]`). It is replaced atomically
//! (`.tmp` + rename) so a crash never leaves a half-written tombstone list. Anything unreadable, including a
//! corrupt or non-array file, reads as "no tombstones", like TS.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The tombstone file name inside the app data directory.
pub const PENDING_DELETES_FILE: &str = "pending-deletes.json";

#[derive(Debug)]
pub struct PendingDeletes {
    file: PathBuf,
    /// Serializes read-modify-write (TS runs these on one thread).
    lock: Mutex<()>,
}

impl PendingDeletes {
    /// Tombstones under `dir` (the app data directory, the parent of `recordings`).
    pub fn new(dir: impl AsRef<Path>) -> PendingDeletes {
        PendingDeletes {
            file: dir.as_ref().join(PENDING_DELETES_FILE),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    /// Tombstoned meeting ids in the order they were confirmed. An unreadable file is empty.
    pub fn ids(&self) -> Vec<String> {
        let Ok(raw) = std::fs::read_to_string(&self.file) else {
            return Vec::new();
        };
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Array(v)) => v
                .into_iter()
                .filter_map(|x| match x {
                    serde_json::Value::String(s) => Some(s),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn has(&self, id: &str) -> bool {
        self.ids().iter().any(|x| x == id)
    }

    /// Writes the tombstone for `id` (idempotent).
    pub fn add(&self, id: &str) -> std::io::Result<()> {
        let _g = self.guard();
        let mut ids = self.ids();
        ids.push(id.to_string());
        self.write(ids)
    }

    /// Drops the tombstone for `id`. `Ok(false)`: there was none (the delete already went through, or never
    /// was scheduled).
    pub fn remove(&self, id: &str) -> std::io::Result<bool> {
        let _g = self.guard();
        let ids = self.ids();
        if !ids.iter().any(|x| x == id) {
            return Ok(false);
        }
        self.write(ids.into_iter().filter(|x| x != id).collect())?;
        Ok(true)
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `JSON.stringify([...new Set(ids)])` to `<file>.tmp`, then rename over the file. Owner-only (0600) on Unix,
    /// like the other app-data stores (ADR 0016); the content is the same as TS writes.
    fn write(&self, ids: Vec<String>) -> std::io::Result<()> {
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<String> = ids.into_iter().filter(|x| seen.insert(x.clone())).collect();
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", self.file.display()));
        let body = serde_json::to_string(&ids).map_err(std::io::Error::other)?;
        {
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            use std::io::Write;
            let mut f = o.open(&tmp)?;
            f.write_all(body.as_bytes())?;
            f.sync_all()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // A `.tmp` left by an older build keeps its mode through `open`; make it owner-only before it lands.
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_and_ts_format() {
        let d = tempfile::tempdir().unwrap();
        let pd = PendingDeletes::new(d.path());
        assert!(pd.ids().is_empty());
        pd.add("a").unwrap();
        pd.add("b").unwrap();
        pd.add("a").unwrap();
        assert_eq!(pd.ids(), ["a", "b"]);
        assert_eq!(std::fs::read_to_string(pd.path()).unwrap(), r#"["a","b"]"#);
        assert!(pd.has("b"));
        assert!(pd.remove("a").unwrap());
        assert!(!pd.remove("a").unwrap());
        assert_eq!(pd.ids(), ["b"]);
        assert!(!d.path().join("pending-deletes.json.tmp").exists());
    }

    #[test]
    fn unreadable_is_empty() {
        let d = tempfile::tempdir().unwrap();
        let pd = PendingDeletes::new(d.path());
        for bad in ["{nope", "{\"a\":1}", "\"x\"", ""] {
            std::fs::write(pd.path(), bad).unwrap();
            assert!(pd.ids().is_empty(), "{bad}");
        }
        std::fs::write(pd.path(), r#"["x",1,null,"y"]"#).unwrap();
        assert_eq!(pd.ids(), ["x", "y"]);
    }

    #[cfg(unix)]
    #[test]
    fn owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let pd = PendingDeletes::new(d.path().join("nested"));
        pd.add("a").unwrap();
        let mode = std::fs::metadata(pd.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
