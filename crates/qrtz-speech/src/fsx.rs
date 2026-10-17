//! Owner-only file helpers (ADR 0046: staging 0700/0600, installed versions 0500/0400, state 0600).
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub fn mkdir_private(p: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(p)?;
    // Tighten a dir that already existed with looser bits.
    let mode = fs::metadata(p)?.permissions().mode() & 0o777;
    if mode != 0o700 && mode != 0o500 {
        fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn open_private_append(p: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(p)
}

/// temp + fsync + rename, 0600.
pub fn write_private_atomic(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = p.parent().expect("parent");
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        p.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, p)?;
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Installed versions are read-only: files 0400 (the entry 0500), dirs 0500.
pub fn make_read_only(dir: &Path, entry: &str) -> std::io::Result<()> {
    fn walk(d: &Path, exec: &Path) -> std::io::Result<()> {
        for e in fs::read_dir(d)? {
            let e = e?;
            let p = e.path();
            let ft = e.file_type()?;
            if ft.is_dir() {
                walk(&p, exec)?;
            } else if ft.is_file() {
                fs::set_permissions(
                    &p,
                    fs::Permissions::from_mode(if p == exec { 0o500 } else { 0o400 }),
                )?;
            }
        }
        fs::set_permissions(d, fs::Permissions::from_mode(0o500))
    }
    walk(dir, &dir.join(entry))
}

/// Remove a tree that may contain read-only dirs.
pub fn remove_tree(p: &Path) -> std::io::Result<()> {
    if !p.exists() {
        return Ok(());
    }
    fn writable(p: &Path) -> std::io::Result<()> {
        let md = fs::symlink_metadata(p)?;
        if md.is_dir() {
            fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
            for e in fs::read_dir(p)? {
                writable(&e?.path())?;
            }
        }
        Ok(())
    }
    writable(p)?;
    fs::remove_dir_all(p)
}

pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

pub fn sha256_file(p: &Path) -> std::io::Result<(u64, String)> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        n += k as u64;
        h.update(&buf[..k]);
    }
    Ok((n, hex::encode(h.finalize())))
}
