//! ADR 0220 phase 1: nano's notes are plain `.md` files in a user-visible vault folder (the source of truth); a
//! SQLite file outside the vault is a derived index (titles, FTS, note → meeting links) that can be deleted and
//! rebuilt from the files at any time.
//!
//! - **Confinement**: every path is vault-relative and resolved by [`Vault::resolve`]: absolute paths, `..`, and
//!   anything a symlink would take outside the (canonical) vault root are refused; symlinked files and folders are
//!   never followed by the scanner and never written through. Only the host opens a `Vault`; no helper gets a path.
//! - **Files**: YAML front matter carries a stable `id` (and the note's `meetings: [..]`); other front-matter lines
//!   are kept verbatim. Writes are atomic (a hidden temp file in the same folder, fsync, rename, fsync the folder),
//!   so a kill mid-save leaves either the old or the new file, never a torn one.
//! - **Index**: [`Vault::rescan`] compares `(mtime, size)` per file with the index and reparses only what changed
//!   (a hash guards against a touch without an edit). It is what the host's poll watcher calls.
//! - **No silent overwrite**: [`Vault::save`] takes the hash the editor loaded; if the file changed on disk since,
//!   it fails with [`VaultError::Conflict`] unless the caller forces it, and a forced save first keeps the other
//!   version as `<name> (conflict <time>).md`.
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Notes larger than this are indexed by title only and refused by `save` (a vault is notes, not data dumps).
pub const MAX_NOTE_BYTES: usize = 1 << 20;
/// The hidden folder `trash` moves notes into (recoverable; phase 1 does not use the macOS Trash).
pub const TRASH_DIR: &str = ".trash";
const INDEX_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("{0}")]
    Invalid(String),
    #[error("path is outside the notes folder")]
    Escape,
    #[error("note {0} not found")]
    NotFound(String),
    #[error("the note changed outside qrtz")]
    Conflict { current_hash: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}
pub type VResult<T> = std::result::Result<T, VaultError>;

/// A note as parsed from its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteFile {
    pub id: Option<String>,
    pub meetings: Vec<String>,
    /// Front-matter lines other than `id` / `meetings`, verbatim and in order.
    pub extra: Vec<String>,
    pub body: String,
}

/// A row of `doc:list`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocSummary {
    pub id: String,
    pub title: String,
    pub path: String,
    pub updated_at: i64,
    pub meetings: Vec<String>,
}

/// `doc:get`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Doc {
    pub id: String,
    pub title: String,
    pub path: String,
    pub updated_at: i64,
    pub meetings: Vec<String>,
    pub body: String,
    pub hash: String,
}

pub fn hash_bytes(b: &[u8]) -> String {
    let d = Sha256::digest(b);
    d.iter().take(16).map(|x| format!("{x:02x}")).collect()
}

/// A new stable note id: millisecond time + 64 random bits (sortable, unique per vault in practice; a duplicate
/// found by the scanner is re-issued anyway).
pub fn new_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    format!("{ms:012x}{:016x}", rand::random::<u64>())
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn parse_list(v: &str) -> Vec<String> {
    let v = v.trim();
    let inner = v.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(v);
    inner
        .split(',')
        .map(|x| x.trim().trim_matches(|c| c == '"' || c == '\''))
        .filter(|x| valid_id(x))
        .map(str::to_owned)
        .collect()
}

/// Parse a note file. No front matter = all body. Only `id:` and `meetings:` are interpreted.
pub fn parse_note(text: &str) -> NoteFile {
    let mut out = NoteFile {
        id: None,
        meetings: vec![],
        extra: vec![],
        body: text.to_owned(),
    };
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return out;
    };
    // The closing fence: a line that is exactly `---`.
    let mut end = None;
    let mut pos = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            end = Some((pos, pos + line.len()));
            break;
        }
        pos += line.len();
    }
    let Some((fm_end, body_start)) = end else {
        return out;
    };
    let fm = &rest[..fm_end];
    let mut lines = fm.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(v) = line.strip_prefix("id:") {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            if valid_id(v) && out.id.is_none() {
                out.id = Some(v.to_owned());
                continue;
            }
            out.extra.push(line.to_owned());
        } else if let Some(v) = line.strip_prefix("meetings:") {
            let mut ms = parse_list(v);
            if v.trim().is_empty() {
                while let Some(n) = lines.peek() {
                    let t = n.trim_start();
                    if let Some(item) = t.strip_prefix("- ") {
                        ms.extend(parse_list(item));
                        lines.next();
                    } else {
                        break;
                    }
                }
            }
            for m in ms {
                if !out.meetings.contains(&m) {
                    out.meetings.push(m);
                }
            }
        } else {
            out.extra.push(line.to_owned());
        }
    }
    out.body = rest[body_start..].to_owned();
    out
}

/// Render a note file. Front matter is written only when there is something to put in it.
pub fn render_note(n: &NoteFile) -> String {
    let mut s = String::new();
    if n.id.is_some() || !n.meetings.is_empty() || !n.extra.is_empty() {
        s.push_str("---\n");
        if let Some(id) = &n.id {
            s.push_str(&format!("id: {id}\n"));
        }
        if !n.meetings.is_empty() {
            s.push_str(&format!("meetings: [{}]\n", n.meetings.join(", ")));
        }
        for l in &n.extra {
            s.push_str(l);
            s.push('\n');
        }
        s.push_str("---\n");
    }
    s.push_str(&n.body);
    s
}

/// The list title: the first non-empty line (a leading `#` heading marker dropped), else the file name.
pub fn title_of(body: &str, stem: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .map_or_else(|| stem.to_owned(), clip)
}

fn clip(s: &str) -> String {
    if s.chars().count() > 120 {
        let mut c: String = s.chars().take(119).collect();
        c.push('…');
        c
    } else {
        s.to_owned()
    }
}

/// A file name from a title: no separators, control or leading dot characters, at most 80 chars.
pub fn file_stem_for(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s
        .trim_start_matches('.')
        .trim()
        .chars()
        .take(80)
        .collect::<String>();
    if s.is_empty() {
        "untitled".to_owned()
    } else {
        s
    }
}

/// Write `bytes` to `path` atomically: hidden temp file in the same folder, fsync, rename, fsync the folder.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?;
    let tmp = dir.join(format!(
        ".{}.qrtz-tmp-{}-{:08x}",
        name.to_string_lossy(),
        std::process::id(),
        rand::random::<u32>()
    ));
    let r = (|| {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

fn is_temp(name: &str) -> bool {
    name.starts_with('.') && name.contains(".qrtz-tmp-")
}

fn mtime_ns(m: &fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// What a rescan changed (ids), for `doc:changed`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub changed: BTreeSet<String>,
    pub removed: BTreeSet<String>,
}
impl ScanReport {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
    pub fn ids(&self) -> Vec<String> {
        self.changed.union(&self.removed).cloned().collect()
    }
}

pub struct Vault {
    root: PathBuf,
    db: Connection,
}

impl Vault {
    /// Open (creating) the vault folder and its index. `root` must be absolute; it is canonicalized, so a vault
    /// reached through a symlink is confined to where the link points.
    pub fn open(root: &Path, index: &Path) -> VResult<Vault> {
        if !root.is_absolute() {
            return Err(VaultError::Invalid(
                "the notes folder must be an absolute path".into(),
            ));
        }
        fs::create_dir_all(root)?;
        let root = fs::canonicalize(root)?;
        if !fs::metadata(&root)?.is_dir() {
            return Err(VaultError::Invalid("the notes folder is not a folder".into()));
        }
        if let Some(p) = index.parent() {
            fs::create_dir_all(p)?;
        }
        let db = match Self::open_index(index) {
            Ok(db) => db,
            Err(e) => {
                // A corrupt or foreign index is only a cache: drop it and rebuild from the files.
                let _ = e;
                let _ = fs::remove_file(index);
                Self::open_index(index)?
            }
        };
        let mut v = Vault { root, db };
        v.rescan()?;
        Ok(v)
    }

    fn open_index(index: &Path) -> VResult<Connection> {
        let db = Connection::open(index)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        let ok: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if ok != "ok" {
            return Err(VaultError::Invalid(format!("index check: {ok}")));
        }
        let ver: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if ver != INDEX_VERSION {
            db.execute_batch(
                "DROP TABLE IF EXISTS docs; DROP TABLE IF EXISTS docs_fts; DROP TABLE IF EXISTS doc_meetings;",
            )?;
            db.execute_batch(&format!(
                "CREATE TABLE docs (id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, title TEXT NOT NULL,
                   mtime_ns INTEGER NOT NULL, size INTEGER NOT NULL, hash TEXT NOT NULL);
                 CREATE INDEX docs_mtime ON docs(mtime_ns DESC);
                 CREATE VIRTUAL TABLE docs_fts USING fts5(id UNINDEXED, title, body, tokenize='porter unicode61');
                 CREATE TABLE doc_meetings (doc_id TEXT NOT NULL, meeting_id TEXT NOT NULL,
                   PRIMARY KEY (doc_id, meeting_id));
                 CREATE INDEX doc_meetings_m ON doc_meetings(meeting_id);
                 PRAGMA user_version = {INDEX_VERSION};"
            ))?;
        }
        Ok(db)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a vault-relative path, refusing absolute paths, `..`, and symlink escapes. The file need not exist;
    /// its parent folder must, and must resolve inside the vault.
    pub fn resolve(&self, rel: &str) -> VResult<PathBuf> {
        let p = Path::new(rel);
        if rel.is_empty() || p.is_absolute() || rel.contains('\0') {
            return Err(VaultError::Escape);
        }
        for c in p.components() {
            if !matches!(c, Component::Normal(_)) {
                return Err(VaultError::Escape);
            }
        }
        let full = self.root.join(p);
        let parent = full.parent().ok_or(VaultError::Escape)?;
        let cparent = fs::canonicalize(parent).map_err(|_| VaultError::Escape)?;
        if !cparent.starts_with(&self.root) {
            return Err(VaultError::Escape);
        }
        let target = cparent.join(full.file_name().ok_or(VaultError::Escape)?);
        if let Ok(m) = fs::symlink_metadata(&target) {
            if m.file_type().is_symlink() {
                return Err(VaultError::Escape);
            }
        }
        Ok(target)
    }

    fn rel_of(&self, full: &Path) -> Option<String> {
        full.strip_prefix(&self.root)
            .ok()
            .map(|r| r.to_string_lossy().into_owned())
    }

    /// Every `.md` file under the vault: hidden entries, the trash and symlinks are skipped (never followed).
    fn walk(&self) -> Vec<(String, fs::Metadata)> {
        let mut out = vec![];
        let mut stack = vec![self.root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(m) = fs::symlink_metadata(e.path()) else {
                    continue;
                };
                if m.file_type().is_symlink() {
                    continue;
                }
                if m.is_dir() {
                    stack.push(e.path());
                } else if m.is_file() && name.to_ascii_lowercase().ends_with(".md") {
                    if let Some(rel) = self.rel_of(&e.path()) {
                        out.push((rel, m));
                    }
                }
            }
        }
        out
    }

    /// Bring the index in line with the files. Unchanged `(mtime, size)` costs one stat; a changed file is read,
    /// given an id if it has none (or a duplicate one), and reindexed. Leftover temp files from a crash are removed.
    pub fn rescan(&mut self) -> VResult<ScanReport> {
        self.sweep_temps();
        let files = self.walk();
        let mut known: HashMap<String, (String, i64, i64, String)> = HashMap::new();
        {
            let mut st = self
                .db
                .prepare("SELECT path, id, mtime_ns, size, hash FROM docs")?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?),
                ))
            })?;
            for r in rows {
                let (p, v) = r?;
                known.insert(p, v);
            }
        }
        let mut rep = ScanReport::default();
        let present: BTreeSet<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        // Removed first, so an id that moved (rename outside qrtz) is free for its new path.
        let gone: Vec<(String, String)> = known
            .iter()
            .filter(|(p, _)| !present.contains(p.as_str()))
            .map(|(p, v)| (p.clone(), v.0.clone()))
            .collect();
        for (p, id) in gone {
            self.drop_row(&id)?;
            known.remove(&p);
            rep.removed.insert(id);
        }
        for (rel, m) in files {
            let (mt, sz) = (mtime_ns(&m), m.len() as i64);
            if let Some((_, kmt, ksz, _)) = known.get(&rel) {
                if *kmt == mt && *ksz == sz {
                    continue;
                }
            }
            if let Some(id) = self.index_file(&rel, known.get(&rel).map(|k| k.3.as_str()))? {
                rep.removed.remove(&id);
                rep.changed.insert(id);
            }
        }
        Ok(rep)
    }

    fn sweep_temps(&self) {
        let mut stack = vec![self.root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Ok(m) = fs::symlink_metadata(e.path()) else {
                    continue;
                };
                if m.file_type().is_symlink() {
                    continue;
                }
                if m.is_dir() && !name.starts_with('.') {
                    stack.push(e.path());
                } else if m.is_file() && is_temp(&name) {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }

    fn drop_row(&self, id: &str) -> VResult<()> {
        self.db.execute("DELETE FROM docs WHERE id=?", [id])?;
        self.db.execute("DELETE FROM docs_fts WHERE id=?", [id])?;
        self.db.execute("DELETE FROM doc_meetings WHERE doc_id=?", [id])?;
        Ok(())
    }

    /// Read and index one file. Returns its id, or None when its bytes were already indexed (a touch).
    fn index_file(&mut self, rel: &str, known_hash: Option<&str>) -> VResult<Option<String>> {
        let full = self.resolve(rel)?;
        let bytes = fs::read(&full)?;
        let hash = hash_bytes(&bytes);
        let m = fs::metadata(&full)?;
        if known_hash == Some(hash.as_str()) {
            self.db.execute(
                "UPDATE docs SET mtime_ns=?, size=? WHERE path=?",
                params![mtime_ns(&m), m.len() as i64, rel],
            )?;
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut note = parse_note(&text);
        // An id another indexed file already holds (a copied note) or no id at all: issue one and write it into
        // the front matter (body bytes untouched).
        let clash = match &note.id {
            Some(id) => self
                .db
                .query_row("SELECT path FROM docs WHERE id=?", [id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
                .is_some_and(|p| p != rel && self.root.join(&p).exists()),
            None => true,
        };
        let (bytes, hash, m) = if clash {
            note.id = Some(new_id());
            let out = render_note(&note);
            atomic_write(&full, out.as_bytes())?;
            let m = fs::metadata(&full)?;
            (out.into_bytes(), String::new(), m)
        } else {
            (bytes, hash, m)
        };
        let hash = if hash.is_empty() { hash_bytes(&bytes) } else { hash };
        let id = note.id.clone().unwrap_or_default();
        let stem = Path::new(rel)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = title_of(&note.body, &stem);
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM docs WHERE id=? OR path=?", params![id, rel])?;
        tx.execute("DELETE FROM docs_fts WHERE id=?", [&id])?;
        tx.execute("DELETE FROM doc_meetings WHERE doc_id=?", [&id])?;
        tx.execute(
            "INSERT INTO docs (id,path,title,mtime_ns,size,hash) VALUES (?,?,?,?,?,?)",
            params![id, rel, title, mtime_ns(&m), m.len() as i64, hash],
        )?;
        let body = if note.body.len() > MAX_NOTE_BYTES {
            ""
        } else {
            note.body.as_str()
        };
        tx.execute(
            "INSERT INTO docs_fts (id,title,body) VALUES (?,?,?)",
            params![id, title, body],
        )?;
        for mid in &note.meetings {
            tx.execute(
                "INSERT OR IGNORE INTO doc_meetings VALUES (?,?)",
                params![id, mid],
            )?;
        }
        tx.commit()?;
        Ok(Some(id))
    }

    /// Drop the index and rebuild it from the files.
    pub fn rebuild(&mut self) -> VResult<usize> {
        self.db
            .execute_batch("DELETE FROM docs; DELETE FROM docs_fts; DELETE FROM doc_meetings;")?;
        self.rescan()?;
        self.count()
    }

    pub fn count(&self) -> VResult<usize> {
        Ok(self
            .db
            .query_row("SELECT COUNT(*) FROM docs", [], |r| r.get::<_, i64>(0))? as usize)
    }

    fn meetings_of(&self, id: &str) -> VResult<Vec<String>> {
        let mut st = self
            .db
            .prepare_cached("SELECT meeting_id FROM doc_meetings WHERE doc_id=? ORDER BY meeting_id")?;
        let r = st
            .query_map([id], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(r)
    }

    /// Newest first. `query` = FTS over title and body; `meeting_id` = only notes linked to that meeting.
    pub fn list(
        &self,
        query: Option<&str>,
        meeting_id: Option<&str>,
        limit: u32,
    ) -> VResult<Vec<DocSummary>> {
        let limit = limit.clamp(1, 10_000) as i64;
        let fts = query.and_then(crate::fts::to_fts_query);
        let mut sql = String::from("SELECT d.id, d.title, d.path, d.mtime_ns FROM docs d");
        let mut args: Vec<String> = vec![];
        let mut wh = vec![];
        if let Some(q) = &fts {
            wh.push("d.id IN (SELECT id FROM docs_fts WHERE docs_fts MATCH ?)");
            args.push(q.clone());
        } else if query.is_some_and(|q| !q.trim().is_empty()) {
            return Ok(vec![]);
        }
        if let Some(m) = meeting_id {
            wh.push("d.id IN (SELECT doc_id FROM doc_meetings WHERE meeting_id=?)");
            args.push(m.to_owned());
        }
        if !wh.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wh.join(" AND "));
        }
        sql.push_str(" ORDER BY d.mtime_ns DESC LIMIT ?");
        let mut st = self.db.prepare(&sql)?;
        let mut p: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a as &dyn rusqlite::ToSql).collect();
        p.push(&limit);
        let rows = st
            .query_map(p.as_slice(), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, title, path, mt)| {
                Ok(DocSummary {
                    meetings: self.meetings_of(&id)?,
                    id,
                    title,
                    path,
                    updated_at: mt / 1_000_000,
                })
            })
            .collect()
    }

    fn path_of(&self, id: &str) -> VResult<String> {
        self.db
            .query_row("SELECT path FROM docs WHERE id=?", [id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| VaultError::NotFound(id.to_owned()))
    }

    /// Read a note from disk (the file, not the index, is the truth), reindexing it if it moved on.
    pub fn get(&mut self, id: &str) -> VResult<Doc> {
        let rel = self.path_of(id)?;
        let full = self.resolve(&rel)?;
        let bytes = match fs::read(&full) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.rescan()?;
                return Err(VaultError::NotFound(id.to_owned()));
            }
            Err(e) => return Err(e.into()),
        };
        let hash = hash_bytes(&bytes);
        let known: String = self
            .db
            .query_row("SELECT hash FROM docs WHERE id=?", [id], |r| r.get(0))?;
        if known != hash {
            self.index_file(&rel, Some(&known))?;
        }
        let note = parse_note(&String::from_utf8_lossy(&bytes));
        let m = fs::metadata(&full)?;
        let stem = full
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Doc {
            id: id.to_owned(),
            title: title_of(&note.body, &stem),
            path: rel,
            updated_at: mtime_ns(&m) / 1_000_000,
            meetings: note.meetings,
            body: note.body,
            hash,
        })
    }

    fn unique_rel(&self, dir: &str, stem: &str) -> String {
        let base = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        for n in 1..10_000 {
            let name = if n == 1 {
                format!("{stem}.md")
            } else {
                format!("{stem} ({n}).md")
            };
            let rel = format!("{base}{name}");
            if fs::symlink_metadata(self.root.join(&rel)).is_err() {
                return rel;
            }
        }
        format!("{base}{stem} {}.md", new_id())
    }

    /// A new note at the vault's top level: `title` becomes its `# heading` and file name.
    pub fn create(
        &mut self,
        title: Option<&str>,
        meeting_id: Option<&str>,
        body: Option<&str>,
    ) -> VResult<Doc> {
        let title = title.map(str::trim).filter(|t| !t.is_empty());
        let stem = title.map_or_else(|| "untitled".to_owned(), file_stem_for);
        let rel = self.unique_rel("", &stem);
        let full = self.resolve(&rel)?;
        let id = new_id();
        let note = NoteFile {
            id: Some(id.clone()),
            meetings: meeting_id
                .filter(|m| valid_id(m))
                .map(|m| vec![m.to_owned()])
                .unwrap_or_default(),
            extra: vec![],
            body: body
                .map(str::to_owned)
                .unwrap_or_else(|| title.map(|t| format!("# {t}\n\n")).unwrap_or_default()),
        };
        atomic_write(&full, render_note(&note).as_bytes())?;
        self.index_file(&rel, None)?;
        self.get(&id)
    }

    /// Save a note's body. `base_hash` is the hash the editor loaded; a file changed on disk since then is a
    /// conflict unless `force`, and a forced save keeps the other version as a conflict copy first. Front matter
    /// (id, meetings, other keys) is kept as on disk. An `untitled` file is renamed after its title on save.
    pub fn save(&mut self, id: &str, body: &str, base_hash: &str, force: bool) -> VResult<Doc> {
        if body.len() > MAX_NOTE_BYTES {
            return Err(VaultError::Invalid("note is too large".into()));
        }
        let rel = self.path_of(id)?;
        let full = self.resolve(&rel)?;
        let disk = fs::read(&full).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VaultError::NotFound(id.to_owned())
            } else {
                e.into()
            }
        })?;
        let current = hash_bytes(&disk);
        let mut note = parse_note(&String::from_utf8_lossy(&disk));
        if current != base_hash {
            if !force {
                return Err(VaultError::Conflict {
                    current_hash: current,
                });
            }
            let stem = full
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let stamp = chrono_like(now_ms());
            let crel = self.unique_rel(dir_of(&rel), &format!("{stem} (conflict {stamp})"));
            let mut other = note.clone();
            other.id = None; // the copy is a new note; the scanner gives it its own id
            atomic_write(&self.resolve(&crel)?, render_note(&other).as_bytes())?;
        }
        if note.id.as_deref() != Some(id) {
            note.id = Some(id.to_owned());
        }
        note.body = body.to_owned();
        atomic_write(&full, render_note(&note).as_bytes())?;
        let mut rel = rel;
        let stem = Path::new(&rel)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = title_of(body, &stem);
        if (stem == "untitled" || stem.starts_with("untitled (")) && title != stem && !body.trim().is_empty()
        {
            let nrel = self.unique_rel(dir_of(&rel), &file_stem_for(&title));
            fs::rename(&full, self.resolve(&nrel)?)?;
            self.db
                .execute("UPDATE docs SET path=? WHERE id=?", params![nrel, id])?;
            rel = nrel;
        }
        self.index_file(&rel, None)?;
        self.rescan()?;
        self.get(id)
    }

    fn set_meetings(&mut self, id: &str, f: impl FnOnce(&mut Vec<String>)) -> VResult<Doc> {
        let rel = self.path_of(id)?;
        let full = self.resolve(&rel)?;
        let mut note = parse_note(&String::from_utf8_lossy(&fs::read(&full)?));
        f(&mut note.meetings);
        note.id = Some(id.to_owned());
        atomic_write(&full, render_note(&note).as_bytes())?;
        self.index_file(&rel, None)?;
        self.get(id)
    }

    pub fn link_meeting(&mut self, id: &str, meeting_id: &str) -> VResult<Doc> {
        if !valid_id(meeting_id) {
            return Err(VaultError::Invalid("bad meeting id".into()));
        }
        self.set_meetings(id, |m| {
            if !m.iter().any(|x| x == meeting_id) {
                m.push(meeting_id.to_owned());
            }
        })
    }

    pub fn unlink_meeting(&mut self, id: &str, meeting_id: &str) -> VResult<Doc> {
        self.set_meetings(id, |m| m.retain(|x| x != meeting_id))
    }

    /// Move a note into the vault's hidden `.trash` folder (recoverable from Finder).
    pub fn trash(&mut self, id: &str) -> VResult<()> {
        let rel = self.path_of(id)?;
        let full = self.resolve(&rel)?;
        let tdir = self.root.join(TRASH_DIR);
        fs::create_dir_all(&tdir)?;
        let name = full
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut dst = tdir.join(&name);
        if dst.exists() {
            dst = tdir.join(format!("{} {}", now_ms(), name));
        }
        fs::rename(&full, &dst)?;
        self.drop_row(id)?;
        Ok(())
    }

    /// The oldest note linked to `meeting_id`, if any.
    pub fn note_for_meeting(&self, meeting_id: &str) -> VResult<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT d.id FROM docs d JOIN doc_meetings m ON m.doc_id=d.id WHERE m.meeting_id=? ORDER BY d.id LIMIT 1",
                [meeting_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The mini HUD's quick add: one line appended to the meeting's note (created, titled after the meeting, when
    /// it has none). The user's text only: no timestamp or summary is added (ADR 0220).
    pub fn append_to_meeting(&mut self, meeting_id: &str, meeting_title: &str, line: &str) -> VResult<Doc> {
        let line = line.trim();
        if line.is_empty() {
            return Err(VaultError::Invalid("empty note".into()));
        }
        let id = match self.note_for_meeting(meeting_id)? {
            Some(id) => id,
            None => {
                // Carries the migration marker: the meeting's old rows (the HUD also stores the line there) must
                // never be migrated into a second note.
                let id = self.create(Some(meeting_title), Some(meeting_id), None)?.id;
                let rel = self.path_of(&id)?;
                let full = self.resolve(&rel)?;
                let mut n = parse_note(&String::from_utf8_lossy(&fs::read(&full)?));
                n.extra.push(format!("{MIGRATED_KEY}: {meeting_id}"));
                atomic_write(&full, render_note(&n).as_bytes())?;
                self.index_file(&rel, None)?;
                id
            }
        };
        let d = self.get(&id)?;
        let mut body = d.body.clone();
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&format!("- {line}\n"));
        self.save(&id, &body, &d.hash, false)
    }
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

/// `YYYY-MM-DD HHMM` (UTC) without a date crate.
fn chrono_like(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}{:02}", rem / 3600, (rem % 3600) / 60)
}

/// One meeting's old timestamped notes, for [`migrate_meeting_notes`].
pub struct LegacyMeeting {
    pub id: String,
    pub title: String,
    /// `(at_ms, text)` in recording order.
    pub lines: Vec<(i64, String)>,
}

/// The marker key a migrated note carries in its front matter, so a migration never runs twice for a meeting
/// even when the index (or the data dir marker) is lost.
pub const MIGRATED_KEY: &str = "qrtz-migrated-from";

fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// ADR 0220 migration: each meeting with old notes becomes one `.md` note linked to it (`- [m:ss] text` lines,
/// the user's existing data). Meetings without notes produce nothing. Idempotent: a meeting whose note already
/// exists (by its front-matter marker) is skipped. The old rows are never touched. Returns the meetings migrated.
pub fn migrate_meeting_notes(v: &mut Vault, meetings: &[LegacyMeeting]) -> VResult<Vec<String>> {
    let mut done: BTreeSet<String> = BTreeSet::new();
    // Markers live in the files, so read them from disk (the index doesn't keep extra front matter).
    for (rel, _) in v.walk() {
        if let Ok(full) = v.resolve(&rel) {
            if let Ok(t) = fs::read_to_string(full) {
                for l in parse_note(&t).extra {
                    if let Some(m) = l.strip_prefix(&format!("{MIGRATED_KEY}:")) {
                        done.insert(m.trim().to_owned());
                    }
                }
            }
        }
    }
    let mut out = vec![];
    for m in meetings {
        if m.lines.is_empty() || done.contains(&m.id) || !valid_id(&m.id) {
            continue;
        }
        let title = if m.title.trim().is_empty() {
            "meeting notes"
        } else {
            m.title.trim()
        };
        let mut body = format!("# {title}\n\n");
        for (at, text) in &m.lines {
            let mut first = true;
            for l in text.lines() {
                if first {
                    body.push_str(&format!("- [{}] {}\n", clock(*at), l));
                    first = false;
                } else {
                    body.push_str(&format!("  {l}\n"));
                }
            }
        }
        let stem = file_stem_for(title);
        let rel = v.unique_rel("", &stem);
        let note = NoteFile {
            id: Some(new_id()),
            meetings: vec![m.id.clone()],
            extra: vec![format!("{MIGRATED_KEY}: {}", m.id)],
            body,
        };
        atomic_write(&v.resolve(&rel)?, render_note(&note).as_bytes())?;
        v.index_file(&rel, None)?;
        done.insert(m.id.clone());
        out.push(m.id.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_round_trip_keeps_unknown_keys_and_body_bytes() {
        let t = "---\nid: abc123\ntags: [x, y]\nmeetings:\n  - m1\n  - m2\n---\n# Hi\r\nbody\n";
        let n = parse_note(t);
        assert_eq!(n.id.as_deref(), Some("abc123"));
        assert_eq!(n.meetings, vec!["m1", "m2"]);
        assert_eq!(n.extra, vec!["tags: [x, y]"]);
        assert_eq!(n.body, "# Hi\r\nbody\n");
        let r = render_note(&n);
        assert_eq!(parse_note(&r), n);
        assert!(r.ends_with("# Hi\r\nbody\n"));
    }

    #[test]
    fn no_front_matter_is_all_body() {
        let n = parse_note("---not fm\nhello");
        assert_eq!(n.id, None);
        assert_eq!(n.body, "---not fm\nhello");
        let n = parse_note("---\nid: x\nno close");
        assert_eq!(n.id, None);
    }

    #[test]
    fn titles() {
        assert_eq!(title_of("\n\n# Plan\nx", "f"), "Plan");
        assert_eq!(title_of("first line\n# later", "f"), "first line");
        assert_eq!(title_of("## Sub\n", "f"), "Sub");
        assert_eq!(title_of("  \n", "file"), "file");
        assert_eq!(file_stem_for("a/b:c?"), "a b c");
        assert_eq!(file_stem_for("..."), "untitled");
    }

    #[test]
    fn dates() {
        assert_eq!(chrono_like(0), "1970-01-01 0000");
        assert_eq!(chrono_like(1_792_432_009_000), "2026-10-19 1746");
    }
}
