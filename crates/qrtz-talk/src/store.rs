//! `talk.json`: history + dictionary, the same file and format as Electron's `TalkService` (spec/fixtures/talk).
//!
//! - Read: an entry that doesn't parse is dropped and the rest kept (zod `safeParse` per entry). A file that is
//!   not JSON at all is moved aside to `talk.json.corrupt-<ms>` (owner-only) before anything is written, so a
//!   damaged file is never silently overwritten (Electron replaced it on the next save).
//! - Write: atomic and owner-only (T3/T4): a crash-left `.tmp` is removed first (the create mode only applies
//!   when the write creates the file), the tmp is created 0600, written, fsynced and renamed over the target. A
//!   failed write (disk full) removes its partial tmp and leaves the previous file untouched; the caller keeps
//!   the in-memory state and the next write carries it.
//! - A stale `.tmp` is also removed at load (8aeb5d6): it can hold dictated text and must not linger.
use crate::contract::{DictionaryEntry, TalkEntry};
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};

pub const MAX_HISTORY: usize = 500;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Store {
    pub history: Vec<TalkEntry>,
    pub dictionary: Vec<DictionaryEntry>,
}

/// File operations, injectable so tests can fail a write like a full disk would.
pub trait StoreIo: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn write_private_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn quarantine(&self, path: &Path, to: &Path) -> io::Result<()>;
    fn remove_stale_tmp(&self, path: &Path) -> io::Result<()>;
}

pub struct FsIo;

pub fn tmp_path(file: &Path) -> PathBuf {
    let mut t = file.as_os_str().to_owned();
    t.push(".tmp");
    PathBuf::from(t)
}

fn remove_if_exists(p: &Path) -> io::Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

impl StoreIo for FsIo {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }
    fn write_private_atomic(&self, file: &Path, bytes: &[u8]) -> io::Result<()> {
        write_private_atomic(file, bytes)
    }
    fn quarantine(&self, path: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(path, to)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
    fn remove_stale_tmp(&self, path: &Path) -> io::Result<()> {
        remove_if_exists(&tmp_path(path))
    }
}

/// Same contract as qrtz-app `write_private_atomic` (qrtz-app depends on this crate, not the other way), plus:
/// a failed write removes its own partial tmp.
pub fn write_private_atomic(file: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = tmp_path(file);
    remove_if_exists(&tmp)?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let r = (|| {
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, file)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadOutcome {
    Missing,
    Loaded {
        dropped: usize,
    },
    /// Not JSON (or not an object): moved aside to this path (None = the move failed too).
    Corrupt {
        moved_to: Option<PathBuf>,
    },
    Unreadable(String),
}

/// A dictation from the removed Deepgram engine keeps its text and reads as `elevenlabs`: still a cloud engine (so
/// history never claims it stayed on the device), and the entry isn't dropped (`legacyTalkEntry` in TS).
fn legacy_entry(mut h: Value) -> Value {
    if let Some(o) = h
        .as_object_mut()
        .filter(|o| o.get("engine").is_some_and(|e| e == "deepgram"))
    {
        o.insert("engine".into(), Value::from("elevenlabs"));
    }
    h
}

/// Parse a talk.json body the way TS does. `None` = not a JSON object (corrupt).
pub fn parse(bytes: &[u8]) -> Option<(Store, usize)> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if !v.is_object() {
        return None;
    }
    let items = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let mut dropped = 0;
    let history: Vec<TalkEntry> = items("history")
        .into_iter()
        .map(legacy_entry)
        .filter_map(|h| serde_json::from_value(h).map_err(|_| dropped += 1).ok())
        .collect();
    let dictionary: Vec<DictionaryEntry> = items("dictionary")
        .into_iter()
        .filter_map(|d| {
            let e = serde_json::from_value::<DictionaryEntry>(d)
                .ok()
                .and_then(|d| d.normalized());
            if e.is_none() {
                dropped += 1;
            }
            e
        })
        .collect();
    Some((Store { history, dictionary }, dropped))
}

/// `JSON.stringify(store)`: `{"history":[…],"dictionary":[…]}`.
/// Field order is TS's (a struct, not a `json!` map, which would sort keys).
pub fn serialize(s: &Store) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct Body<'a> {
        history: &'a [TalkEntry],
        dictionary: &'a [DictionaryEntry],
    }
    serde_json::to_vec(&Body {
        history: &s.history,
        dictionary: &s.dictionary,
    })
    .unwrap_or_default()
}

pub fn load(io: &dyn StoreIo, path: &Path, now_ms: f64) -> (Store, LoadOutcome) {
    let _ = io.remove_stale_tmp(path);
    let bytes = match io.read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (Store::default(), LoadOutcome::Missing),
        Err(e) => return (Store::default(), LoadOutcome::Unreadable(e.kind().to_string())),
    };
    match parse(&bytes) {
        Some((s, dropped)) => (s, LoadOutcome::Loaded { dropped }),
        None => {
            let mut to = path.as_os_str().to_owned();
            to.push(format!(".corrupt-{}", now_ms as i64));
            let to = PathBuf::from(to);
            let moved = io.quarantine(path, &to).ok().map(|_| to);
            (Store::default(), LoadOutcome::Corrupt { moved_to: moved })
        }
    }
}
