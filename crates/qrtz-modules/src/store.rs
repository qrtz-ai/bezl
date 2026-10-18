//! The global content-addressed store (ADR 0157 §4): `<root>/store/<sha256>`, one file per distinct blob (every
//! package file and every package tree), shared by every package and version that contains those bytes.
//!
//! - A blob enters only after its size and sha256 match ([`Store::insert_file`]): partial downloads live in
//!   `store/.tmp/` until then.
//! - Before a blob is linked into a package dir it is hashed again ([`Store::link`]): a store file that changed on
//!   disk is never linked; it is removed and fetched again.
//! - Linking: on macOS `std::fs::copy`, which clones on APFS (`fclonefileat`: no extra disk until a side writes,
//!   and the clone's mode is its own) and copies elsewhere. On other systems a hard link for read-only files, else a
//!   copy (Linux runs the tests this way).
//! - Store files are 0444 in a 0700 dir; `prune` removes blobs no locked package's tree names (refcounted from
//!   `qrtz.lock`).
use crate::fsutil;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkMethod {
    /// APFS clone (macOS; falls back to a copy on volumes without clones).
    Clone,
    Hardlink,
    Copy,
}

#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn path(&self, sha256: &str) -> PathBuf {
        debug_assert!(crate::manifest::is_sha256(sha256));
        self.dir.join(sha256)
    }
    /// Partial downloads.
    pub fn tmp(&self) -> PathBuf {
        self.dir.join(".tmp")
    }

    /// Present with the right size (cheap; [`Store::verify`] hashes).
    pub fn has(&self, sha256: &str, size: u64) -> bool {
        std::fs::metadata(self.path(sha256)).is_ok_and(|m| m.is_file() && m.len() == size)
    }

    /// Present, right size and right hash. A mismatching file is removed.
    pub fn verify(&self, sha256: &str, size: u64) -> std::io::Result<bool> {
        let p = self.path(sha256);
        match fsutil::sha256_file(&p) {
            Ok((n, h)) if n == size && h == sha256 => Ok(true),
            Ok(_) => {
                let _ = fsutil::set_mode(&p, 0o600);
                std::fs::remove_file(&p)?;
                Ok(false)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Move a downloaded file into the store if (and only if) it matches.
    pub fn insert_file(&self, src: &Path, sha256: &str, size: u64) -> std::io::Result<bool> {
        let (n, h) = fsutil::sha256_file(src)?;
        if n != size || h != sha256 {
            let _ = std::fs::remove_file(src);
            return Ok(false);
        }
        fsutil::mkdir_private(&self.dir)?;
        fsutil::set_mode(src, 0o444)?;
        std::fs::rename(src, self.path(sha256))?;
        Ok(true)
    }

    /// Small blobs (trees, `qrtz.json`) from memory.
    pub fn insert_bytes(&self, bytes: &[u8], sha256: &str) -> std::io::Result<bool> {
        if crate::sha256_hex(bytes) != sha256 {
            return Ok(false);
        }
        if self.has(sha256, bytes.len() as u64) {
            return Ok(true);
        }
        let tmp = self.tmp();
        fsutil::mkdir_private(&tmp)?;
        let t = tmp.join(format!("{sha256}.in-{}", std::process::id()));
        std::fs::write(&t, bytes)?;
        self.insert_file(&t, sha256, bytes.len() as u64)
    }

    /// A small blob's bytes, checked.
    pub fn read_verified(&self, sha256: &str, max: usize) -> std::io::Result<Option<Vec<u8>>> {
        let p = self.path(sha256);
        match std::fs::metadata(&p) {
            Ok(m) if m.len() as usize > max => return Ok(None),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        }
        let b = std::fs::read(&p)?;
        Ok((crate::sha256_hex(&b) == sha256).then_some(b))
    }

    /// Verify the blob, then place it at `dest` (which must not exist) with mode 0400, or 0500 if `executable`.
    pub fn link(
        &self,
        sha256: &str,
        size: u64,
        dest: &Path,
        executable: bool,
    ) -> std::io::Result<LinkMethod> {
        if !self.verify(sha256, size)? {
            return Err(std::io::Error::other(format!(
                "{sha256}: not in the store (or changed on disk)"
            )));
        }
        if let Some(parent) = dest.parent() {
            fsutil::mkdir_private(parent)?;
        }
        let src = self.path(sha256);
        let method = link_or_copy(&src, dest, executable)?;
        if method != LinkMethod::Hardlink {
            fsutil::set_mode(dest, if executable { 0o500 } else { 0o400 })?;
        }
        Ok(method)
    }

    /// Every blob in the store: (sha256, size).
    pub fn list(&self) -> Vec<(String, u64)> {
        let Ok(d) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        let mut v: Vec<(String, u64)> = d
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_str()?.to_owned();
                crate::manifest::is_sha256(&n).then(|| (n, e.metadata().map(|m| m.len()).unwrap_or(0)))
            })
            .collect();
        v.sort();
        v
    }

    pub fn remove(&self, sha256: &str) -> std::io::Result<()> {
        let p = self.path(sha256);
        let _ = fsutil::set_mode(&p, 0o600);
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(target_os = "macos")]
fn link_or_copy(src: &Path, dest: &Path, _executable: bool) -> std::io::Result<LinkMethod> {
    // std's macOS copy is fclonefileat first (an APFS clone), then fcopyfile.
    std::fs::copy(src, dest)?;
    Ok(LinkMethod::Clone)
}

#[cfg(not(target_os = "macos"))]
fn link_or_copy(src: &Path, dest: &Path, executable: bool) -> std::io::Result<LinkMethod> {
    // A hard link shares the store's 0444 inode, so only read-only files take one; executables get their own copy.
    if !executable && std::fs::hard_link(src, dest).is_ok() {
        return Ok(LinkMethod::Hardlink);
    }
    std::fs::copy(src, dest)?;
    Ok(LinkMethod::Copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_matching_bytes_enter_and_a_changed_blob_is_never_linked() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path().join("store"));
        let b = b"hello store".to_vec();
        let h = crate::sha256_hex(&b);
        assert!(!s.insert_bytes(b"other", &h).unwrap());
        assert!(s.insert_bytes(&b, &h).unwrap());
        let out = d.path().join("pkg/a.txt");
        s.link(&h, b.len() as u64, &out, false).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b);
        // Tamper with the store file: the next link refuses and the bad blob is gone.
        fsutil::set_mode(&s.path(&h), 0o600).unwrap();
        std::fs::write(s.path(&h), b"hello storX").unwrap();
        assert!(s
            .link(&h, b.len() as u64, &d.path().join("pkg/b.txt"), false)
            .is_err());
        assert!(!s.path(&h).exists());
    }
}
