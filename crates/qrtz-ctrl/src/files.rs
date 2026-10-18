//! Slice 7 (ADR 0113): downloads cleanup. A `files.*` move expands into concrete file operations on a folder,
//! planned here (pure over a directory listing), run by [`crate::fs_exec::FsExecutor`] as one batch action, and
//! verified by re-reading the folder. No UI at all, so it can't touch focus, the cursor or the frontmost app.
//!
//! The default (`files.tidy`, `files.sort_by_kind`) only **moves** top-level files into kind folders inside the same
//! folder (`images`, `documents`, …), creating the folder if needed: never overwrites, never deletes, every op has an
//! inverse ([`undo_ops`]). Only `files.trash_old` (the task says trash / delete / old) moves installers older than
//! 30 days to the Trash, and that move is irreversible in the plan, so it waits at the confirm gate with a `file`
//! preview. Paths are referenced in the log by `f<sha256(path)[..6]>` refs; readable names stay in memory.
//!
//! `files.empty_trash` (ADR 0122) plans one [`Planned::Delete`] per top-level item of the Trash, skipping what
//! was trashed in the last [`SETTLE`] (it may still be put back), locked items and the Finder's own hidden files.
//! It is the one plan that deletes, so it is always gated and has no undo.
//!
//! `files.desktop_screenshots` (ADR 0124) moves the screenshots and screen recordings macOS saved on the Desktop
//! (matched by the system's own file names and kind) into `Desktop/screenshots`; nothing else on the Desktop moves.
//! `files.trash_screenshots` (the task says trash / delete) sends the same files to the Trash behind the gate.
use crate::fs_exec::MemStore;
use crate::ladder::FsOp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where ctrl's file routes act. `None` on the service: file routes hold as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRoots {
    pub downloads: PathBuf,
    /// The user's Trash (`~/.Trash` on macOS); a move there is undone by moving back.
    pub trash: PathBuf,
    /// The Desktop (`~/Desktop` on macOS): only its screenshots and screen recordings are ctrl's (ADR 0124).
    pub desktop: PathBuf,
}

impl FileRoots {
    /// `~/Downloads`, `~/.Trash` and `~/Desktop` on macOS; nothing elsewhere (tests pass a temp dir).
    pub fn default_for_os() -> Option<Self> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(Self {
            downloads: home.join("Downloads"),
            trash: home.join(".Trash"),
            desktop: home.join("Desktop"),
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
    MakeDir {
        dir: PathBuf,
    },
    Move {
        src: PathBuf,
        dst_dir: PathBuf,
    },
    Trash {
        src: PathBuf,
    },
    /// Delete one top-level item of the Trash for good (ADR 0122).
    Delete {
        src: PathBuf,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilePlan {
    pub ops: Vec<Planned>,
}

impl FilePlan {
    /// The files this plan sends to the Trash or deletes from it (the gate's `file` preview).
    pub fn trashed(&self) -> Vec<&Path> {
        self.ops
            .iter()
            .filter_map(|o| match o {
                Planned::Trash { src } | Planned::Delete { src } => Some(src.as_path()),
                _ => None,
            })
            .collect()
    }

    /// The plan deletes something (emptying the Trash): it can't be undone.
    pub fn deletes(&self) -> bool {
        self.ops.iter().any(|o| matches!(o, Planned::Delete { .. }))
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
                Planned::Delete { src } => FsOp::Delete { src: put(src) },
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
                Planned::Delete { src } => src.symlink_metadata().is_ok(),
            })
            .count() as i64
    }
}

/// Plan the cleanup of `roots.downloads` for a `route.files.*` move. Pure over the listing and `now`.
pub fn plan_cleanup(roots: &FileRoots, mv: &str, now: SystemTime) -> std::io::Result<FilePlan> {
    if mv == EMPTY_TRASH_MOVE {
        return plan_empty_trash(roots, now);
    }
    if mv == DESKTOP_SCREENSHOTS_MOVE || mv == TRASH_SCREENSHOTS_MOVE {
        return plan_desktop_screenshots(roots, mv == TRASH_SCREENSHOTS_MOVE, now);
    }
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

/// The move that sorts the Desktop's screenshots and screen recordings into [`SCREENSHOTS_DIR`] (ADR 0124).
pub const DESKTOP_SCREENSHOTS_MOVE: &str = "route.files.desktop_screenshots";
/// The move that sends them to the Trash instead (only when the task says so; irreversible, gated).
pub const TRASH_SCREENSHOTS_MOVE: &str = "route.files.trash_screenshots";
/// The Desktop folder screenshots go into.
pub const SCREENSHOTS_DIR: &str = "screenshots";

/// A screenshot or screen recording by the name and kind macOS gives it: `Screenshot 2026-10-18 at 6.37.12 AM.png`
/// (`Screen Shot …` before macOS 10.14 / 13), `Screen Recording 2026-10-18 at 6.37.12 AM.mov`, also with a ` (2)`
/// or `-1` suffix. The prefix is followed by a `yyyy-mm-dd` date; the kind must match the prefix (an image for a
/// screenshot, a movie for a recording), so `screenshot ideas.txt` or a renamed file stays put.
pub fn is_screen_capture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(ext) = Path::new(&lower).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let image = matches!(
        ext,
        "png" | "jpg" | "jpeg" | "heic" | "tiff" | "gif" | "bmp" | "pdf"
    );
    let movie = matches!(ext, "mov" | "mp4");
    let rest = if let Some(r) = lower
        .strip_prefix("screenshot ")
        .or_else(|| lower.strip_prefix("screen shot "))
    {
        if !image {
            return false;
        }
        r
    } else if let Some(r) = lower.strip_prefix("screen recording ") {
        if !movie {
            return false;
        }
        r
    } else {
        return false;
    };
    let b = rest.as_bytes();
    b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

/// Plan the Desktop's screenshots (ADR 0124): every settled top-level screenshot / screen recording moves into
/// `Desktop/screenshots` (created if needed; a same-named file there keeps the new one where it is), or with
/// `trash` to the Trash (a same-named item there keeps it too). Everything else on the Desktop stays. Pure over the
/// listing and `now`; at most [`MAX_EMPTY`] per run.
pub fn plan_desktop_screenshots(
    roots: &FileRoots,
    trash: bool,
    now: SystemTime,
) -> std::io::Result<FilePlan> {
    let mut found: Vec<(PathBuf, String)> = Vec::new();
    for e in std::fs::read_dir(&roots.desktop)? {
        let e = e?;
        let meta = e.path().symlink_metadata()?;
        if !meta.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        // A screenshot still being written (a recording in progress) has a fresh mtime: it waits.
        let fresh = meta
            .modified()
            .ok()
            .filter(|t| *t <= now)
            .is_none_or(|t| now.duration_since(t).unwrap_or_default() < SETTLE);
        if fresh || !is_screen_capture(&name) || locked(&meta) {
            continue;
        }
        found.push((e.path(), name));
    }
    found.sort();
    found.truncate(MAX_EMPTY);
    let dir = roots.desktop.join(SCREENSHOTS_DIR);
    let mut plan = FilePlan::default();
    if trash {
        for (path, name) in found {
            if !roots.trash.join(&name).exists() {
                plan.ops.push(Planned::Trash { src: path });
            }
        }
        return Ok(plan);
    }
    if dir.exists() && !dir.is_dir() {
        return Ok(plan);
    }
    for (path, name) in found {
        if dir.join(&name).exists() {
            continue;
        }
        if plan.ops.is_empty() && !dir.exists() {
            plan.ops.push(Planned::MakeDir { dir: dir.clone() });
        }
        plan.ops.push(Planned::Move {
            src: path,
            dst_dir: dir.clone(),
        });
    }
    Ok(plan)
}

/// The move that empties the Trash (ADR 0122).
pub const EMPTY_TRASH_MOVE: &str = "route.files.empty_trash";

/// Most items one empty-the-trash run deletes; the rest wait for the next run (a bounded preview and log).
pub const MAX_EMPTY: usize = 500;

/// Hidden names the Finder keeps in the Trash for itself (`.DS_Store`, `.localized`): never ctrl's to delete.
fn finder_own(name: &str) -> bool {
    name.starts_with('.')
}

/// When the item last changed: the newer of its modification and inode-change time. Moving a file into the Trash
/// changes the second (a rename), so a file edited long ago but trashed a minute ago reads as fresh. A time after
/// `now` (a clock that moved) isn't evidence and is ignored.
fn changed_at(meta: &std::fs::Metadata, now: SystemTime) -> Option<SystemTime> {
    let m = meta.modified().ok().filter(|t| *t <= now);
    #[cfg(unix)]
    let c = {
        use std::os::unix::fs::MetadataExt;
        let secs = meta.ctime();
        u64::try_from(secs)
            .ok()
            .map(|s| {
                std::time::UNIX_EPOCH
                    + Duration::from_secs(s)
                    + Duration::from_nanos(meta.ctime_nsec().max(0) as u64)
            })
            .filter(|t| *t <= now)
    };
    #[cfg(not(unix))]
    let c: Option<SystemTime> = None;
    m.max(c)
}

/// Locked in the Finder (the user immutable flag, or the system one). Only macOS has the flags.
fn locked(meta: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const UF_IMMUTABLE: u32 = 0x0000_0002;
        const SF_IMMUTABLE: u32 = 0x0002_0000;
        meta.st_flags() & (UF_IMMUTABLE | SF_IMMUTABLE) != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = meta;
        false
    }
}

/// A folder with anything locked inside (looked at down to a bounded depth / count; past the bound it counts as
/// locked, so the folder stays). A half-deleted folder is worse than one that waits.
fn holds_locked(dir: &Path, budget: &mut usize, depth: usize) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return true;
    };
    for e in rd {
        let Ok(e) = e else { return true };
        if *budget == 0 || depth == 0 {
            return true;
        }
        *budget -= 1;
        let Ok(meta) = e.path().symlink_metadata() else {
            return true;
        };
        if locked(&meta) || (meta.file_type().is_dir() && holds_locked(&e.path(), budget, depth - 1)) {
            return true;
        }
    }
    false
}

/// Plan emptying the Trash (ADR 0122): one delete per top-level item, in name order, at most [`MAX_EMPTY`]. Left
/// alone: anything trashed (or changed) in the last [`SETTLE`], locked items or folders holding one, and the
/// Finder's hidden files. Pure over the listing and `now`. A missing Trash is an empty one.
pub fn plan_empty_trash(roots: &FileRoots, now: SystemTime) -> std::io::Result<FilePlan> {
    let rd = match std::fs::read_dir(&roots.trash) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FilePlan::default()),
        Err(e) => return Err(e),
    };
    let mut keep = Vec::new();
    for e in rd {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if finder_own(&name) {
            continue;
        }
        let meta = e.path().symlink_metadata()?;
        let fresh = changed_at(&meta, now).is_none_or(|t| now.duration_since(t).unwrap_or_default() < SETTLE);
        if fresh || locked(&meta) {
            continue;
        }
        if meta.file_type().is_dir() && holds_locked(&e.path(), &mut 20_000, 32) {
            continue;
        }
        keep.push(e.path());
    }
    keep.sort();
    keep.truncate(MAX_EMPTY);
    Ok(FilePlan {
        ops: keep.into_iter().map(|src| Planned::Delete { src }).collect(),
    })
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
            Planned::Delete { .. } => Err("fs.irreversible".into()),
        })
        .collect()
}

fn back(from: &Path, to: &Path) -> Result<(), String> {
    if to.symlink_metadata().is_ok() {
        return Err("fs.exists".into());
    }
    std::fs::rename(from, to).map_err(|e| format!("fs.rename:{:?}", e.kind()))
}

/// Every kind folder [`kind_of`] can create.
pub const KINDS: [&str; 6] = ["images", "documents", "archives", "installers", "audio", "video"];

/// Why one file of a done batch can't be put back (ADR 0117).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoWhy {
    /// It isn't where ctrl put it any more: moved, renamed or deleted since.
    Moved,
    /// Something with its name is back where it came from: putting it back would overwrite.
    Occupied,
}

/// One file of the batch that stays where it is, by its log ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoSkip {
    pub file: String,
    pub why: UndoWhy,
}

/// What putting a done batch back would do, read from the disk now. Paths are memory only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UndoPlan {
    /// (where the file is now, where it came from), in the batch's order.
    pub back: Vec<(PathBuf, PathBuf)>,
    /// (ref, path) of the files that go back (for labels).
    pub refs: Vec<String>,
    /// Folders the batch created: removed after the moves only if they are empty.
    pub dirs: Vec<PathBuf>,
    pub skipped: Vec<UndoSkip>,
    /// Files in the batch (moves + trashes).
    pub files: usize,
}

/// Resolve a done `FsOp::Batch` against the folder as it is now. The log holds only `f<sha256[..6]>` refs, so
/// each file is found again by hashing the names in its kind folder (or the Trash): nothing is guessed, a ref
/// that matches nothing is `moved`.
pub fn plan_undo(op: &FsOp, roots: &FileRoots) -> UndoPlan {
    let FsOp::Batch { ops } = op else {
        return UndoPlan::default();
    };
    let mut dirs: std::collections::BTreeMap<String, PathBuf> = KINDS
        .iter()
        .map(|k| roots.downloads.join(k))
        .chain([roots.desktop.join(SCREENSHOTS_DIR)])
        .map(|d| (path_ref(&d), d))
        .collect();
    dirs.insert(path_ref(&roots.trash), roots.trash.clone());
    // (the name now, where it came from): the ref says which folder it left (Downloads or the Desktop, ADR 0124).
    let find = |dir: &Path, src: &str| -> Option<(String, PathBuf)> {
        std::fs::read_dir(dir).ok()?.filter_map(Result::ok).find_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_file = e.path().symlink_metadata().is_ok_and(|m| m.file_type().is_file());
            if !is_file {
                return None;
            }
            [&roots.downloads, &roots.desktop]
                .into_iter()
                .map(|root| root.join(&name))
                .find(|from| path_ref(from) == src)
                .map(|from| (name.clone(), from))
        })
    };
    let mut plan = UndoPlan::default();
    for o in ops {
        let (src, dir) = match o {
            FsOp::MakeDir { dir } => {
                if let Some(d) = dirs.get(&dir.0) {
                    plan.dirs.push(d.clone());
                }
                continue;
            }
            FsOp::MoveInto { src, dst_dir } => (src, dirs.get(&dst_dir.0)),
            FsOp::Trash { src } => (src, Some(&roots.trash)),
            FsOp::Batch { .. } | FsOp::Delete { .. } => continue,
        };
        plan.files += 1;
        let skip = |why| UndoSkip {
            file: src.0.clone(),
            why,
        };
        match dir.and_then(|d| find(d, &src.0).map(|(n, from)| (d.join(&n), from))) {
            None => plan.skipped.push(skip(UndoWhy::Moved)),
            Some((_, to)) if to.symlink_metadata().is_ok() => plan.skipped.push(skip(UndoWhy::Occupied)),
            Some((from, to)) => {
                plan.back.push((from, to));
                plan.refs.push(src.0.clone());
            }
        }
    }
    plan
}

/// Put back every file of `plan.back`, all or nothing: a failed move puts the ones already moved where they were.
/// Then the batch's folders are removed if they are empty (a folder with anything else in it stays). Returns how
/// many files went back.
pub fn run_undo(plan: &UndoPlan) -> Result<usize, String> {
    for (i, (from, to)) in plan.back.iter().enumerate() {
        if let Err(e) = back(from, to) {
            for (f, t) in plan.back[..i].iter().rev() {
                let _ = back(t, f);
            }
            return Err(e);
        }
    }
    for d in plan.dirs.iter().rev() {
        let _ = std::fs::remove_dir(d);
    }
    Ok(plan.back.len())
}
