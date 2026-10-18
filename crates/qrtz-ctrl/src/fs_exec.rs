//! Rung 1: plain file operations. Downloads cleanup needs no UI, no Accessibility and no Apple Events; only the
//! Files and Folders grant for ~/Downloads, which the user gives in the foreground.
//!
//! Paths live in a [`ContentStore`] and the log only sees [`ContentRef`]s. Moves never overwrite, and "delete"
//! means move to the Trash (the macOS executor uses `FileManager.trashItem`; here it is a configured directory),
//! so every file action has an inverse, except [`FsOp::Delete`] (ADR 0122): emptying the Trash removes items that
//! are already in it, only after the confirm gate, and can't be undone.
use crate::ladder::{ContentRef, FsOp, Route};
use crate::runner::Executor;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub trait ContentStore {
    fn path(&self, r: &ContentRef) -> Option<PathBuf>;
}

#[derive(Debug, Default, Clone)]
pub struct MemStore(pub BTreeMap<ContentRef, PathBuf>);

impl MemStore {
    pub fn put(&mut self, id: &str, p: impl Into<PathBuf>) -> ContentRef {
        let r = ContentRef(id.to_string());
        self.0.insert(r.clone(), p.into());
        r
    }
}

impl ContentStore for MemStore {
    fn path(&self, r: &ContentRef) -> Option<PathBuf> {
        self.0.get(r).cloned()
    }
}

pub struct FsExecutor<S: ContentStore> {
    pub store: S,
    pub trash_dir: PathBuf,
}

fn move_into(src: &Path, dir: &Path) -> Result<(), String> {
    let name = src.file_name().ok_or("fs.no_name")?;
    if !dir.is_dir() {
        return Err("fs.no_dir".into());
    }
    let dst = dir.join(name);
    // Never overwrite: an existing name is an error the planner sees, not data loss.
    if dst.symlink_metadata().is_ok() {
        return Err("fs.exists".into());
    }
    std::fs::rename(src, &dst).map_err(|e| format!("fs.rename:{:?}", e.kind()))
}

/// Remove one item that sits directly in `trash` (ADR 0122). Anything else is refused: the unlink can't reach
/// outside the Trash. A symlink is removed itself (never followed); a folder goes with everything in it
/// (`remove_dir_all` doesn't follow symlinks inside it either).
fn delete_in(p: &Path, trash: &Path) -> Result<(), String> {
    if p.parent() != Some(trash) || p.file_name().is_none() {
        return Err("fs.outside_trash".into());
    }
    let meta = p
        .symlink_metadata()
        .map_err(|e| format!("fs.stat:{:?}", e.kind()))?;
    let r = if meta.file_type().is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    r.map_err(|e| format!("fs.delete:{:?}", e.kind()))
}

impl<S: ContentStore> FsExecutor<S> {
    fn run(&mut self, op: &FsOp) -> Result<(), String> {
        match op {
            FsOp::MakeDir { dir } => {
                let d = self.store.path(dir).ok_or("fs.ref")?;
                if d.symlink_metadata().is_ok() {
                    return Err("fs.exists".into());
                }
                std::fs::create_dir(&d).map_err(|e| format!("fs.mkdir:{:?}", e.kind()))
            }
            FsOp::Batch { ops } => {
                for (i, o) in ops.iter().enumerate() {
                    if let Err(e) = self.run(o) {
                        // Roll back what this batch did, newest first: the folder is as it was.
                        for done in ops[..i].iter().rev() {
                            let _ = self.undo(done);
                        }
                        return Err(e);
                    }
                }
                Ok(())
            }
            FsOp::MoveInto { src, dst_dir } => {
                let (s, d) = (
                    self.store.path(src).ok_or("fs.ref")?,
                    self.store.path(dst_dir).ok_or("fs.ref")?,
                );
                move_into(&s, &d)
            }
            FsOp::Trash { src } => {
                let s = self.store.path(src).ok_or("fs.ref")?;
                std::fs::create_dir_all(&self.trash_dir).map_err(|e| format!("fs.trash:{:?}", e.kind()))?;
                move_into(&s, &self.trash_dir)
            }
            FsOp::Delete { src } => {
                let s = self.store.path(src).ok_or("fs.ref")?;
                delete_in(&s, &self.trash_dir)
            }
        }
    }

    /// The inverse of one done op.
    pub fn undo(&mut self, op: &FsOp) -> Result<(), String> {
        let back = |from: PathBuf, to: PathBuf| -> Result<(), String> {
            if to.symlink_metadata().is_ok() {
                return Err("fs.exists".into());
            }
            std::fs::rename(from, to).map_err(|e| format!("fs.rename:{:?}", e.kind()))
        };
        match op {
            FsOp::MakeDir { dir } => std::fs::remove_dir(self.store.path(dir).ok_or("fs.ref")?)
                .map_err(|e| format!("fs.rmdir:{:?}", e.kind())),
            FsOp::MoveInto { src, dst_dir } => {
                let (s, d) = (
                    self.store.path(src).ok_or("fs.ref")?,
                    self.store.path(dst_dir).ok_or("fs.ref")?,
                );
                back(d.join(s.file_name().ok_or("fs.no_name")?), s)
            }
            FsOp::Trash { src } => {
                let s = self.store.path(src).ok_or("fs.ref")?;
                back(self.trash_dir.join(s.file_name().ok_or("fs.no_name")?), s)
            }
            FsOp::Delete { .. } => Err("fs.irreversible".into()),
            FsOp::Batch { ops } => ops.iter().rev().try_for_each(|o| self.undo(o)),
        }
    }
}

impl<S: ContentStore> Executor for FsExecutor<S> {
    fn execute(&mut self, route: &Route) -> Result<(), String> {
        let Route::Files(op) = route else {
            return Err("fs.not_a_file_route".into());
        };
        self.run(op)
    }
}
