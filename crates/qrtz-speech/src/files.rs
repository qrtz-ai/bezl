//! N73: the store hands out **opened** files, never paths. Every file is opened from a dir fd component by component
//! with `O_NOFOLLOW` (no symlink is ever followed), checked to be a regular file of the manifest size, and hashed
//! from the opened fd itself, so what was hashed is exactly what the module and the health check get (no TOCTOU).
//! N75: at most `MAX_MODEL_FDS` model files are passed to a module; the entry binary and the golden clip are
//! opened for the host only.
use crate::bundle::{FileEntry, Manifest};
use crate::SpeechError;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Model fds inherited by a module (fds 7..7+n). macOS' default soft RLIMIT_NOFILE is 256.
pub use qrtz_speech_types::protocol::MAX_MODEL_FDS;

#[derive(Debug)]
pub struct VerifiedFile {
    /// Manifest path (for the engine's own lookup; never opened by path again).
    pub path: String,
    pub file: File,
    pub size: u64,
}

#[derive(Debug)]
pub struct ModuleFiles {
    /// The module executable (spawned from this fd: `fexecve` on Linux; on macOS by its verified path with the
    /// code requirement checked, see ADR 0046 §6, same-user exclusion).
    pub entry: VerifiedFile,
    /// The golden clip for the health check (host only).
    pub clip: VerifiedFile,
    /// Everything else: passed to the module as fds 7.., at most MAX_MODEL_FDS.
    pub models: Vec<VerifiedFile>,
}

fn cstr(s: &[u8]) -> Result<CString, SpeechError> {
    CString::new(s).map_err(|_| SpeechError::Invalid("nul in path".into()))
}

fn openat(dir: &OwnedFd, name: &[u8], flags: libc::c_int) -> Result<OwnedFd, SpeechError> {
    let c = cstr(name)?;
    // SAFETY: valid dir fd and nul-terminated name; the result is owned below.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            c.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        return Err(match e.raw_os_error() {
            Some(libc::ELOOP) | Some(libc::ENOTDIR) => {
                SpeechError::Invalid("symlink or non-directory in bundle".into())
            }
            Some(libc::ENOENT) => SpeechError::Missing(String::from_utf8_lossy(name).into()),
            _ => SpeechError::Io(e),
        });
    }
    // SAFETY: fd is a fresh, owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Open `dir` itself without following a symlink.
pub fn open_dir(dir: &Path) -> Result<OwnedFd, SpeechError> {
    let c = cstr(dir.as_os_str().as_bytes())?;
    // SAFETY: nul-terminated path; owned below.
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(SpeechError::Io(std::io::Error::last_os_error()));
    }
    // SAFETY: fresh owned fd.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Open one manifest file below `root` (no `..`, no symlinks), check it is regular and hash the opened fd.
pub fn open_verified_file(root: &OwnedFd, f: &FileEntry) -> Result<VerifiedFile, SpeechError> {
    if !crate::bundle::safe_rel_path(&f.path) {
        return Err(SpeechError::Invalid(format!("file path {:?}", f.path)));
    }
    let parts: Vec<&[u8]> = Path::new(&f.path).iter().map(|c| c.as_bytes()).collect();
    let mut dir: Option<OwnedFd> = None;
    for p in &parts[..parts.len() - 1] {
        let base = dir.as_ref().unwrap_or(root);
        dir = Some(openat(base, p, libc::O_RDONLY | libc::O_DIRECTORY)?);
    }
    let base = dir.as_ref().unwrap_or(root);
    let fd = openat(base, parts[parts.len() - 1], libc::O_RDONLY)?;
    let mut file = File::from(fd);
    let md = file.metadata()?;
    if !md.file_type().is_file() {
        return Err(SpeechError::Invalid(format!("{} is not a regular file", f.path)));
    }
    if md.len() != f.size {
        return Err(SpeechError::Checksum(f.path.clone()));
    }
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let k = file.read(&mut buf)?;
        if k == 0 {
            break;
        }
        n += k as u64;
        if n > f.size {
            return Err(SpeechError::Checksum(f.path.clone()));
        }
        h.update(&buf[..k]);
    }
    if n != f.size || hex::encode(h.finalize()) != f.sha256 {
        return Err(SpeechError::Checksum(f.path.clone()));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(VerifiedFile {
        path: f.path.clone(),
        file,
        size: f.size,
    })
}

/// Open and hash every manifest file of the bundle in `dir`.
pub fn open_module_files(dir: &Path, m: &Manifest) -> Result<ModuleFiles, SpeechError> {
    let root = open_dir(dir)?;
    let mut entry = None;
    let mut clip = None;
    let mut models = Vec::new();
    for f in &m.files {
        let v = open_verified_file(&root, f)?;
        if f.path == m.entry {
            entry = Some(v);
        } else if f.path == m.health.clip {
            clip = Some(v);
        } else {
            models.push(v);
        }
    }
    if models.len() > MAX_MODEL_FDS {
        return Err(SpeechError::Invalid("too many model files".into()));
    }
    Ok(ModuleFiles {
        entry: entry.ok_or_else(|| SpeechError::Missing(m.entry.clone()))?,
        clip: clip.ok_or_else(|| SpeechError::Missing(m.health.clip.clone()))?,
        models,
    })
}
