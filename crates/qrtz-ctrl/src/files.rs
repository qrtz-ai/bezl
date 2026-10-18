//! Slice 7 (ADR 0113): downloads cleanup. A `files.*` move expands into concrete file operations on a folder,
//! planned here (pure over a directory listing), run by [`crate::fs_exec::FsExecutor`] as one batch action, and
//! verified by re-reading the folder. No UI at all, so it can't touch focus, the cursor or the frontmost app.
//!
//! The default (`files.tidy`, `files.sort_by_kind`) only **moves** top-level files into kind folders inside the same
//! folder (`images`, `documents`, …), creating the folder if needed: never overwrites, never deletes, every op has an
//! inverse ([`undo_ops`]). Only `files.trash_old` (the task says trash / delete / old) moves installers older than
//! 30 days to the Trash, and that move is irreversible in the plan, so it waits at the confirm gate with a `file`
//! preview. Paths are referenced in the log by `f<sha256(path)[..6]>` refs; readable names stay in memory.
use crate::fs_exec::MemStore;
use crate::ladder::FsOp;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where ctrl's file routes act. `None` on the service: file routes hold as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRoots {
    pub downloads: PathBuf,
    /// The user's Trash (`~/.Trash` on macOS); a move there is undone by moving back.
    pub trash: PathBuf,
}

impl FileRoots {
    /// `~/Downloads` and `~/.Trash` on macOS; nothing elsewhere (tests pass a temp dir).
    pub fn default_for_os() -> Option<Self> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(Self {
            downloads: home.join("Downloads"),
            trash: home.join(".Trash"),
        })
    }
}

/// Files touched within this long might still be downloading or open: left alone.
pub const SETTLE: Duration = Duration::from_secs(10 * 60);
/// Installers older than this are "old" for `files.trash_old`.
pub const OLD_INSTALLER: Duration = Duration::from_secs(30 * 24 * 3600);

/// The kind folder of a file name, or `None` (unknown kinds stay where they are).
pub fn kind_of(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "heic" | "webp" | "svg" | "tiff" | "bmp" => "images",
        "pdf" | "doc" | "docx" | "txt" | "md" | "rtf" | "pages" | "xls" | "xlsx" | "csv" | "numbers"
        | "key" | "ppt" | "pptx" => "documents",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => "archives",
        "dmg" | "pkg" | "mpkg" | "exe" | "msi" => "installers",
        "mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" => "audio",
        "mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" => "video",
        _ => return None,
    })
}

fn partial(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with('.')
        || [".crdownload", ".part", ".download", ".partial"]
            .iter()
            .any(|s| n.ends_with(s))
}

/// The log ref of a path: never the path itself.
pub fn path_ref(p: &Path) -> String {
    format!(
        "f{}",
        hex::encode(&Sha256::digest(p.to_string_lossy().as_bytes())[..6])
    )
}

/// One planned file op with its readable paths (memory only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    MakeDir { dir: PathBuf },
    Move { src: PathBuf, dst_dir: PathBuf },
    Trash { src: PathBuf },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilePlan {
    pub ops: Vec<Planned>,
}

impl FilePlan {
    /// The files this plan sends to the Trash (the gate's `file` preview).
    pub fn trashed(&self) -> Vec<&Path> {
        self.ops
            .iter()
            .filter_map(|o| match o {
                Planned::Trash { src } => Some(src.as_path()),
                _ => None,
            })
            .collect()
    }

    /// Files moved or trashed (makedirs don't count).
    pub fn files(&self) -> usize {
        self.ops
            .iter()
            .filter(|o| !matches!(o, Planned::MakeDir { .. }))
            .count()
    }

    /// The batch as a ladder op plus the store that resolves its refs.
    pub fn to_op(&self, roots: &FileRoots) -> (FsOp, MemStore) {
        let mut store = MemStore::default();
        let mut put = |p: &Path| store.put(&path_ref(p), p);
        let ops = self
            .ops
            .iter()
            .map(|o| match o {
                Planned::MakeDir { dir } => FsOp::MakeDir { dir: put(dir) },
                Planned::Move { src, dst_dir } => FsOp::MoveInto {
                    src: put(src),
                    dst_dir: put(dst_dir),
                },
                Planned::Trash { src } => FsOp::Trash { src: put(src) },
            })
            .collect();
        put(&roots.trash);
        (FsOp::Batch { ops }, store)
    }

    /// How many planned files are not where the plan puts them yet (0 = done). Read from the disk.
    pub fn remaining(&self, roots: &FileRoots) -> i64 {
        self.ops
            .iter()
            .filter(|o| match o {
                Planned::MakeDir { .. } => false,
                Planned::Move { src, dst_dir } => {
                    src.symlink_metadata().is_ok()
                        || !src.file_name().is_some_and(|n| dst_dir.join(n).exists())
                }
                Planned::Trash { src } => {
                    src.symlink_metadata().is_ok()
                        || !src.file_name().is_some_and(|n| roots.trash.join(n).exists())
                }
            })
            .count() as i64
    }
}

/// Plan the cleanup of `roots.downloads` for a `route.files.*` move. Pure over the listing and `now`.
pub fn plan_cleanup(roots: &FileRoots, mv: &str, now: SystemTime) -> std::io::Result<FilePlan> {
    let trash_old = mv == "route.files.trash_old";
    let mut entries: Vec<(PathBuf, String, SystemTime)> = Vec::new();
    for e in std::fs::read_dir(&roots.downloads)? {
        let e = e?;
        let meta = e.path().symlink_metadata()?;
        // Top-level regular files only: folders, symlinks and app bundles stay as they are.
        if !meta.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let mtime = meta.modified().unwrap_or(now);
        if partial(&name) || now.duration_since(mtime).unwrap_or_default() < SETTLE {
            continue;
        }
        entries.push((e.path(), name, mtime));
    }
    entries.sort();
    let mut plan = FilePlan::default();
    let mut made: Vec<PathBuf> = Vec::new();
    for (path, name, mtime) in entries {
        let Some(kind) = kind_of(&name) else { continue };
        let old = now.duration_since(mtime).unwrap_or_default() >= OLD_INSTALLER;
        if trash_old {
            if kind == "installers" && old && !roots.trash.join(&name).exists() {
                plan.ops.push(Planned::Trash { src: path });
            }
            continue;
        }
        let dir = roots.downloads.join(kind);
        // Never overwrite: a same-named file already in the kind folder stays where it is.
        if dir.join(&name).exists() || (dir.exists() && !dir.is_dir()) {
            continue;
        }
        if !dir.exists() && !made.contains(&dir) {
            made.push(dir.clone());
            plan.ops.push(Planned::MakeDir { dir: dir.clone() });
        }
        plan.ops.push(Planned::Move {
            src: path,
            dst_dir: dir,
        });
    }
    Ok(plan)
}

/// The inverse of done ops, newest first: moves go back, created folders are removed if empty.
pub fn undo_ops(done: &[Planned], roots: &FileRoots) -> Vec<Result<(), String>> {
    done.iter()
        .rev()
        .map(|o| match o {
            Planned::MakeDir { dir } => {
                std::fs::remove_dir(dir).map_err(|e| format!("fs.rmdir:{:?}", e.kind()))
            }
            Planned::Move { src, dst_dir } => back(&dst_dir.join(src.file_name().unwrap_or_default()), src),
            Planned::Trash { src } => back(&roots.trash.join(src.file_name().unwrap_or_default()), src),
        })
        .collect()
}

fn back(from: &Path, to: &Path) -> Result<(), String> {
    if to.symlink_metadata().is_ok() {
        return Err("fs.exists".into());
    }
    std::fs::rename(from, to).map_err(|e| format!("fs.rename:{:?}", e.kind()))
}
