//! N132 (ADR 0086): reading a file out of the recordings directory. The directory is owner-only (N25), but what is
//! in it is not all qrtz's own writing: a backup import lands files there, and any process running as the user can
//! put a symlink, a FIFO or a directory under a recording's name. Whoever serves or exports a recording (the
//! `qrtz-media` protocol, a backup export) must read the regular file that is there and nothing it points at:
//! a symlink would hand the renderer or the backup zip any file the user can read; a FIFO would block the reader
//! for good; a directory or device is not audio.
//!
//! [`open_regular`] checks the entry without following it (`lstat`), opens it, and checks that what it opened is the
//! same regular file (device + inode on Unix), so a swap between the check and the open is refused too. Std only.
use std::fs::{self, File};
use std::io;
use std::path::Path;

/// Opens `path` for reading if, and only if, it is a regular file (not a symlink, FIFO, socket, device or
/// directory). Returns the file and its length. `NotFound` stays `NotFound`; everything else refused is
/// `InvalidInput`.
pub fn open_regular(path: &Path) -> io::Result<(File, u64)> {
    let refused = || io::Error::new(io::ErrorKind::InvalidInput, "not a regular file");
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() {
        return Err(refused());
    }
    let f = File::open(path)?;
    let after = f.metadata()?;
    if !after.file_type().is_file() {
        return Err(refused());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (after.dev(), after.ino()) != (before.dev(), before.ino()) {
            return Err(refused());
        }
    }
    Ok((f, after.len()))
}

/// True when `path` is a regular file itself (a symlink to one is not).
pub fn is_regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_files_only() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.wav");
        fs::write(&p, b"RIFF").unwrap();
        let (_, len) = open_regular(&p).unwrap();
        assert_eq!(len, 4);
        assert!(is_regular(&p));
        assert_eq!(
            open_regular(&d.path().join("missing.wav")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::create_dir(d.path().join("dir.wav")).unwrap();
        assert_eq!(
            open_regular(&d.path().join("dir.wav")).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!is_regular(&d.path().join("dir.wav")));
    }
}
