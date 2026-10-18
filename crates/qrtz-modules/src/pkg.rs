//! `.qrtzpkg`, the offline sideload file (ADR 0153 §5): a small mirror of the URL layout in one file, so a
//! sideload goes through the same verification as a download (Bozniak's rule 7).
//!
//! ```text
//! "QRTZPKG1" | toc length (u32 LE) | toc (canonical json) | data
//! toc = {format: 1, module, version, entries: [{path, offset, size}]}
//! ```
//! `path` is a layout path (`v1/root/1.root.json`, `v1/timestamp.json(.sig)`, `v1/index/<sha>.json(.sig)`,
//! `v1/blobs/sha256/<hex>`); offsets are into `data`. Entries are sorted by path, may not overlap, and must lie
//! inside the file. Nothing in a `.qrtzpkg` is trusted by itself: roots chain to the pinned root, the timestamp
//! and index are signed, and every blob is checked against the signed index.
use crate::index::{layout, parse_canonical, IndexError, MAX_INDEX_BYTES};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 8] = b"QRTZPKG1";
pub const FORMAT: u32 = 1;
pub const MAX_TOC_BYTES: usize = 1024 * 1024;
pub const EXTENSION: &str = "qrtzpkg";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub offset: u64,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Toc {
    pub format: u32,
    pub module: String,
    pub version: String,
    pub entries: Vec<Entry>,
}

pub struct Qrtzpkg {
    path: PathBuf,
    pub toc: Toc,
    data_start: u64,
}

fn bad(why: impl Into<String>) -> IndexError {
    IndexError::Invalid("qrtzpkg", why.into())
}

impl Qrtzpkg {
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        let mut f = File::open(path).map_err(|e| bad(e.to_string()))?;
        let len = f.metadata().map_err(|e| bad(e.to_string()))?.len();
        let mut head = [0u8; 12];
        f.read_exact(&mut head).map_err(|_| bad("too short"))?;
        if &head[..8] != MAGIC {
            return Err(bad("not a .qrtzpkg"));
        }
        let n = u32::from_le_bytes(head[8..12].try_into().expect("4 bytes")) as usize;
        if n > MAX_TOC_BYTES {
            return Err(IndexError::TooLarge("qrtzpkg toc"));
        }
        let mut toc = vec![0u8; n];
        f.read_exact(&mut toc).map_err(|_| bad("truncated toc"))?;
        let toc: Toc = parse_canonical("qrtzpkg toc", &toc, MAX_TOC_BYTES)?;
        if toc.format != FORMAT {
            return Err(bad("unsupported format"));
        }
        let data_start = 12 + n as u64;
        let data_len = len - data_start;
        let mut sorted = toc.entries.clone();
        sorted.sort_by_key(|e| e.offset);
        let mut end = 0u64;
        for e in &sorted {
            if !layout::is_valid(&e.path) {
                return Err(bad(format!("{:?}: not a layout path", e.path)));
            }
            if e.offset < end || e.offset.checked_add(e.size).is_none_or(|x| x > data_len) {
                return Err(bad(format!("{}: overlaps or lies outside the file", e.path)));
            }
            end = e.offset + e.size;
        }
        let mut paths: Vec<_> = toc.entries.iter().map(|e| &e.path).collect();
        paths.sort();
        paths.dedup();
        if paths.len() != toc.entries.len() {
            return Err(bad("a path is listed twice"));
        }
        Ok(Self {
            path: path.to_owned(),
            toc,
            data_start,
        })
    }

    pub fn has(&self, p: &str) -> bool {
        self.toc.entries.iter().any(|e| e.path == p)
    }

    fn entry(&self, p: &str) -> Option<&Entry> {
        self.toc.entries.iter().find(|e| e.path == p)
    }

    /// A small document (root, timestamp, index or a signature).
    pub fn get(&self, p: &str) -> Result<Option<Vec<u8>>, IndexError> {
        let Some(e) = self.entry(p) else { return Ok(None) };
        if e.size as usize > MAX_INDEX_BYTES {
            return Err(IndexError::TooLarge("qrtzpkg entry"));
        }
        let mut f = File::open(&self.path).map_err(|e| bad(e.to_string()))?;
        f.seek(SeekFrom::Start(self.data_start + e.offset))
            .map_err(|e| bad(e.to_string()))?;
        let mut b = vec![0u8; e.size as usize];
        f.read_exact(&mut b).map_err(|e| bad(e.to_string()))?;
        Ok(Some(b))
    }

    /// Copy one entry to `dest` (0600), streamed.
    pub fn copy_to(&self, p: &str, dest: &Path) -> Result<bool, IndexError> {
        let Some(e) = self.entry(p) else { return Ok(false) };
        let mut f = File::open(&self.path).map_err(|e| bad(e.to_string()))?;
        f.seek(SeekFrom::Start(self.data_start + e.offset))
            .map_err(|e| bad(e.to_string()))?;
        let mut out = private_create(dest).map_err(|e| bad(e.to_string()))?;
        std::io::copy(&mut f.take(e.size), &mut out).map_err(|e| bad(e.to_string()))?;
        out.sync_all().map_err(|e| bad(e.to_string()))?;
        Ok(true)
    }

    /// sha256 of the whole file: what the user's confirmation binds to.
    pub fn sha256(&self) -> Result<String, IndexError> {
        crate::fsutil::sha256_file(&self.path)
            .map(|(_, h)| h)
            .map_err(|e| bad(e.to_string()))
    }
}

fn private_create(p: &Path) -> std::io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(p)
}

/// Write a `.qrtzpkg` from `(layout path, source file)` pairs (the publisher's `pack`). Deterministic: entries are
/// sorted by path, so the same inputs give the same bytes (and the same sha256 for the confirmation).
pub fn write(dest: &Path, module: &str, version: &str, files: &[(String, PathBuf)]) -> std::io::Result<()> {
    let mut files = files.to_vec();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    let mut entries = vec![];
    let mut off = 0u64;
    for (p, src) in &files {
        if !layout::is_valid(p) {
            return Err(std::io::Error::other(format!("{p}: not a layout path")));
        }
        let size = std::fs::metadata(src)?.len();
        entries.push(Entry {
            path: p.clone(),
            offset: off,
            size,
        });
        off += size;
    }
    let toc = crate::index::canonical(&Toc {
        format: FORMAT,
        module: module.into(),
        version: version.into(),
        entries,
    });
    let tmp = dest.with_extension("qrtzpkg.tmp");
    {
        let mut out = std::io::BufWriter::new(private_create(&tmp)?);
        out.write_all(MAGIC)?;
        out.write_all(&(toc.len() as u32).to_le_bytes())?;
        out.write_all(&toc)?;
        for (_, src) in &files {
            std::io::copy(&mut File::open(src)?, &mut out)?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    }
    std::fs::rename(&tmp, dest)
}
