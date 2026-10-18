//! Owner-only file helpers for the installer (the same rules as ADR 0046's: staging 0700/0600, installed 0500/0400).
use crate::ModuleError;
use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

pub fn mkdir_private(p: &Path) -> std::io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    b.mode(0o700);
    b.create(p)
}

/// temp + fsync + rename, 0600; the parent dir is fsynced so the rename is durable.
pub fn write_private_atomic(p: &Path, bytes: &[u8]) -> Result<(), ModuleError> {
    let dir = p
        .parent()
        .ok_or_else(|| ModuleError::Io("no parent dir".into()))?;
    mkdir_private(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        p.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut o = fs::OpenOptions::new();
        o.create(true).write(true).truncate(true);
        #[cfg(unix)]
        o.mode(0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, p)?;
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

pub fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    return fs::set_permissions(p, fs::Permissions::from_mode(mode));
    #[cfg(not(unix))]
    {
        let _ = (p, mode);
        Ok(())
    }
}

/// Remove a tree that may contain read-only dirs.
pub fn remove_tree(p: &Path) -> std::io::Result<()> {
    let md = match fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if md.is_dir() {
        set_mode(p, 0o700)?;
        for e in fs::read_dir(p)? {
            remove_tree(&e?.path())?;
        }
        fs::remove_dir(p)
    } else {
        fs::remove_file(p)
    }
}

/// sha256 and length of a file, streamed.
pub fn sha256_file(p: &Path) -> std::io::Result<(u64, String)> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((n, hex::encode(h.finalize())))
}
