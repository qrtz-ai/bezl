//! ADR 0218 follow-up N193: `Installer::sweep` cleans `store/.tmp/` (orphan `.part` / `.done`, old `*.in-<pid>` and
//! `*.sideload` leftovers), keeps what a download still wants, and `reclaimable()` counts exactly what `prune()` removes.
use qrtz_modules::index::PackageKind;
use qrtz_modules::install::{CodeChecks, Installer, TmpKeep, TMP_LEFTOVER_AGE};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

struct NoChecks;
impl CodeChecks for NoChecks {
    fn check(&self, _: PackageKind, _: &Path, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn sha(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn installer(root: &Path, keep: TmpKeep) -> Installer {
    Installer::new(root, None, Box::new(NoChecks)).with_tmp_keep(Arc::new(move || keep.clone()))
}

/// A sparse 64 MB part with 4 KB landed at its start, like a cut-short download.
fn part(tmp: &Path, sha: &str) {
    let mut f = std::fs::File::create(tmp.join(format!("{sha}.part"))).unwrap();
    f.set_len(64 << 20).unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    f.write_all(&[7u8; 4096]).unwrap();
    std::fs::write(tmp.join(format!("{sha}.done")), b"0\n").unwrap();
}

fn aged(p: &Path, age: Duration) {
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(SystemTime::now() - age).unwrap();
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let tmp = d.path().join("store/.tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    (d, tmp)
}

#[test]
fn an_orphan_part_and_its_marks_are_removed_and_counted_by_their_allocated_bytes() {
    let (d, tmp) = setup();
    part(&tmp, &sha('a'));
    let i = installer(d.path(), TmpKeep::default());
    let r = i.reclaimable().unwrap();
    assert_eq!(r.blobs, 2, "the part and its marks");
    assert!(r.bytes >= 4096, "the landed range counts: {}", r.bytes);
    assert!(
        r.bytes < 64 << 20,
        "a sparse part counts its blocks, not its length: {}",
        r.bytes
    );
    assert!(
        tmp.join(format!("{}.part", sha('a'))).exists(),
        "reclaimable deletes nothing"
    );
    assert_eq!(i.prune().unwrap(), r, "the button and the prune agree");
    assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
    assert_eq!(i.reclaimable().unwrap().blobs, 0);
}

#[test]
fn a_part_a_manifest_set_names_is_kept() {
    let (d, tmp) = setup();
    part(&tmp, &sha('b'));
    part(&tmp, &sha('c'));
    let keep = TmpKeep {
        sha256: [sha('b')].into(),
        pending: false,
    };
    let i = installer(d.path(), keep);
    assert_eq!(i.prune().unwrap().blobs, 2, "only c's two files");
    assert!(tmp.join(format!("{}.part", sha('b'))).exists());
    assert!(tmp.join(format!("{}.done", sha('b'))).exists());
    assert!(!tmp.join(format!("{}.part", sha('c'))).exists());
}

#[test]
fn a_pending_download_keeps_every_part_it_would_resume() {
    let (d, tmp) = setup();
    part(&tmp, &sha('d'));
    let keep = TmpKeep {
        sha256: Default::default(),
        pending: true,
    };
    let i = installer(d.path(), keep);
    assert_eq!(i.reclaimable().unwrap().blobs, 0);
    assert_eq!(i.prune().unwrap().blobs, 0);
    assert!(tmp.join(format!("{}.part", sha('d'))).exists());
    assert!(tmp.join(format!("{}.done", sha('d'))).exists());
}

#[test]
fn old_insert_and_sideload_leftovers_go_and_fresh_ones_stay() {
    let (d, tmp) = setup();
    let old_in = tmp.join(format!("{}.in-4242", sha('e')));
    let old_side = tmp.join(format!("{}.sideload", sha('f')));
    let new_in = tmp.join(format!("{}.in-77", sha('e')));
    let new_side = tmp.join(format!("{}.sideload", sha('1')));
    let other = tmp.join("notes.txt");
    for p in [&old_in, &old_side, &new_in, &new_side, &other] {
        std::fs::write(p, [1u8; 5000]).unwrap();
    }
    let old = TMP_LEFTOVER_AGE + Duration::from_secs(60);
    for p in [&old_in, &old_side, &other] {
        aged(p, old);
    }
    let i = installer(d.path(), TmpKeep::default());
    let r = i.reclaimable().unwrap();
    assert_eq!(r.blobs, 2);
    assert!(r.bytes >= 2 * 5000, "{}", r.bytes);
    assert_eq!(i.prune().unwrap(), r);
    assert!(!old_in.exists() && !old_side.exists());
    assert!(
        new_in.exists() && new_side.exists(),
        "a running insert or sideload may own them"
    );
    assert!(other.exists(), "nothing the sweep doesn't know");
}

#[test]
fn reclaimable_counts_tmp_leftovers_with_no_package_to_prune() {
    let (d, tmp) = setup();
    part(&tmp, &sha('2'));
    let side = tmp.join(format!("{}.sideload", sha('3')));
    std::fs::write(&side, [0u8; 8192]).unwrap();
    aged(&side, TMP_LEFTOVER_AGE * 2);
    let r = installer(d.path(), TmpKeep::default()).reclaimable().unwrap();
    assert!(r.dirs.is_empty());
    assert_eq!(r.blobs, 3, "so the clean-up button shows");
    assert!(r.bytes >= 4096 + 8192, "{}", r.bytes);
}

#[test]
fn a_part_a_locked_tree_names_is_kept() {
    use qrtz_modules::index::{IndexFile, Tree};
    use qrtz_modules::qrtz_lock::{Locked, QrtzLock, Source};
    let (d, tmp) = setup();
    part(&tmp, &sha('4'));
    part(&tmp, &sha('5'));
    let i = installer(d.path(), TmpKeep::default());
    let tree = Tree::new(vec![IndexFile {
        path: "model.bin".into(),
        size: 64 << 20,
        sha256: sha('4'),
        executable: false,
        mirrors: vec![],
    }]);
    assert!(i.store().insert_bytes(&tree.bytes(), &tree.sha256()).unwrap());
    let mut lock = QrtzLock::default();
    lock.workspace
        .dependencies
        .insert("model.test".into(), "^1.0.0".into());
    lock.workspace
        .resolved
        .insert("model.test".into(), "1.0.0".into());
    lock.packages.insert(
        "model.test@1.0.0".into(),
        Locked {
            kind: PackageKind::Model,
            sha256: tree.sha256(),
            size: tree.size(),
            team_id: "ABCDE12345".into(),
            identifier: None,
            designated_requirement: None,
            dependencies: Default::default(),
            task: None,
            models: vec![],
            source: Source::Pinned,
            stale_revocation: false,
            disabled: None,
            notarization: None,
        },
    );
    std::fs::write(i.lock_path(), lock.to_bytes()).unwrap();
    let r = i.prune().unwrap();
    assert_eq!(r.blobs, 2, "only 5's part and marks: {r:?}");
    assert!(
        tmp.join(format!("{}.part", sha('4'))).exists(),
        "a locked file's part resumes"
    );
    assert!(tmp.join(format!("{}.done", sha('4'))).exists());
    assert!(!tmp.join(format!("{}.part", sha('5'))).exists());
}

/// N193 L1: a `.tmp` that is a symlink (or not a directory) is never swept: the deletes could land anywhere.
#[cfg(unix)]
#[test]
fn a_symlinked_or_non_directory_tmp_is_not_swept() {
    let d = tempfile::tempdir().unwrap();
    let elsewhere = d.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    part(&elsewhere, &sha('e'));
    std::fs::create_dir_all(d.path().join("store")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, d.path().join("store/.tmp")).unwrap();
    let i = installer(d.path(), TmpKeep::default());
    assert_eq!(
        i.reclaimable().unwrap().blobs,
        0,
        "nothing counted through the link"
    );
    assert_eq!(i.prune().unwrap().blobs, 0);
    assert!(elsewhere.join(format!("{}.part", sha('e'))).exists());
    assert!(elsewhere.join(format!("{}.done", sha('e'))).exists());

    let d2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d2.path().join("store")).unwrap();
    std::fs::write(d2.path().join("store/.tmp"), b"not a dir").unwrap();
    let i2 = installer(d2.path(), TmpKeep::default());
    assert_eq!(i2.prune().unwrap().blobs, 0);
    assert!(d2.path().join("store/.tmp").is_file());
}

/// N193 L1 (re-review): a symlinked `store/` is refused too, even when `.tmp` under it is a real directory.
#[cfg(unix)]
#[test]
fn a_symlinked_store_is_not_swept() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real-store");
    std::fs::create_dir_all(real.join(".tmp")).unwrap();
    part(&real.join(".tmp"), &sha('f'));
    std::os::unix::fs::symlink(&real, d.path().join("store")).unwrap();
    let i = installer(d.path(), TmpKeep::default());
    assert_eq!(i.reclaimable().unwrap().blobs, 0);
    assert_eq!(i.prune().unwrap().blobs, 0);
    assert!(real.join(".tmp").join(format!("{}.part", sha('f'))).exists());
}

/// A symlink inside `.tmp` named like a part is never followed or counted (fstatat without following).
#[cfg(unix)]
#[test]
fn a_symlink_inside_tmp_is_left_alone() {
    let (d, tmp) = setup();
    let victim = d.path().join("victim");
    std::fs::write(&victim, b"keep me").unwrap();
    std::os::unix::fs::symlink(&victim, tmp.join(format!("{}.part", sha('9')))).unwrap();
    let i = installer(d.path(), TmpKeep::default());
    assert_eq!(i.prune().unwrap().blobs, 0);
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
}
