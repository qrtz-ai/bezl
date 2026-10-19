//! ADR 0220 phase 1: the notes vault (files are the truth, SQLite is a rebuildable index).
use qrtz_storage::vault::*;
use std::fs;
use std::path::Path;

fn open(dir: &Path) -> Vault {
    Vault::open(&dir.join("notes"), &dir.join("index").join("vault.db")).unwrap()
}

#[test]
fn create_list_get_save_and_search() {
    let t = tempfile::tempdir().unwrap();
    let mut v = open(t.path());
    assert_eq!(v.list(None, None, 100).unwrap(), vec![]);
    let a = v.create(Some("Groceries"), None, None).unwrap();
    assert_eq!(a.title, "Groceries");
    assert_eq!(a.path, "Groceries.md");
    let a = v
        .save(&a.id, "# Groceries\n\nmilk and apples\n", &a.hash, false)
        .unwrap();
    let b = v.create(None, None, None).unwrap();
    assert_eq!(b.path, "untitled.md");
    // An untitled note is named after its title on its first real save.
    let b = v
        .save(&b.id, "Interview prep\nask about the team", &b.hash, false)
        .unwrap();
    assert_eq!(b.path, "Interview prep.md");
    let raw = fs::read_to_string(t.path().join("notes/Interview prep.md")).unwrap();
    assert!(raw.starts_with(&format!("---\nid: {}\n---\n", b.id)), "{raw}");
    let hits = v.list(Some("apples"), None, 10).unwrap();
    assert_eq!(
        hits.iter().map(|d| d.id.clone()).collect::<Vec<_>>(),
        vec![a.id.clone()]
    );
    assert_eq!(v.list(Some("zzz"), None, 10).unwrap(), vec![]);
    assert_eq!(v.list(None, None, 10).unwrap().len(), 2);
}

#[test]
fn no_silent_overwrite_of_an_external_edit() {
    let t = tempfile::tempdir().unwrap();
    let mut v = open(t.path());
    let d = v.create(Some("Plan"), None, None).unwrap();
    let file = t.path().join("notes/Plan.md");
    let mut ext = fs::read_to_string(&file).unwrap();
    ext.push_str("edited in vim\n");
    fs::write(&file, &ext).unwrap();
    // The editor still holds the old hash: refused.
    match v.save(&d.id, "# Plan\nmine\n", &d.hash, false) {
        Err(VaultError::Conflict { .. }) => {}
        r => panic!("expected a conflict, got {r:?}"),
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), ext);
    // A reload sees the outside edit.
    let fresh = v.get(&d.id).unwrap();
    assert!(fresh.body.contains("edited in vim"));
    // "keep mine": forced save keeps theirs as a conflict copy.
    v.save(&d.id, "# Plan\nmine\n", &d.hash, true).unwrap();
    let names: Vec<String> = fs::read_dir(t.path().join("notes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let copy = names
        .iter()
        .find(|n| n.contains("(conflict "))
        .expect("conflict copy");
    assert!(fs::read_to_string(t.path().join("notes").join(copy))
        .unwrap()
        .contains("edited in vim"));
    assert_eq!(v.count().unwrap(), 2, "the copy is its own note with its own id");
    let ids: std::collections::BTreeSet<_> = v
        .list(None, None, 10)
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(ids.len(), 2);
}

#[test]
fn rescan_picks_up_outside_create_rename_delete_and_duplicates() {
    let t = tempfile::tempdir().unwrap();
    let mut v = open(t.path());
    let root = t.path().join("notes");
    fs::write(root.join("From Obsidian.md"), "# Hello\nworld\n").unwrap();
    let r = v.rescan().unwrap();
    assert_eq!(r.changed.len(), 1);
    let id = r.changed.iter().next().unwrap().clone();
    // An id was written into the file's front matter; the body bytes are unchanged.
    let raw = fs::read_to_string(root.join("From Obsidian.md")).unwrap();
    assert_eq!(raw, format!("---\nid: {id}\n---\n# Hello\nworld\n"));
    // Nothing changed: nothing reported.
    assert!(v.rescan().unwrap().is_empty());
    // Renamed outside qrtz: same id, new path.
    fs::rename(root.join("From Obsidian.md"), root.join("Renamed.md")).unwrap();
    v.rescan().unwrap();
    assert_eq!(v.get(&id).unwrap().path, "Renamed.md");
    // Copied: the copy gets its own id.
    fs::copy(root.join("Renamed.md"), root.join("Copy.md")).unwrap();
    v.rescan().unwrap();
    let all = v.list(None, None, 10).unwrap();
    assert_eq!(all.len(), 2);
    assert_ne!(all[0].id, all[1].id);
    assert!(all.iter().any(|d| d.id == id));
    // Deleted outside: gone from the index.
    fs::remove_file(root.join("Copy.md")).unwrap();
    let r = v.rescan().unwrap();
    assert_eq!(r.removed.len(), 1);
    assert_eq!(v.count().unwrap(), 1);
}

#[test]
fn path_escapes_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let v = open(t.path());
    for bad in ["../x.md", "/etc/passwd", "a/../../x.md", "", "./../x.md"] {
        assert!(matches!(v.resolve(bad), Err(VaultError::Escape)), "{bad}");
    }
    // A symlinked folder pointing outside the vault, and a symlinked file.
    let outside = t.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.md"), "secret").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, v.root().join("link")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.md"), v.root().join("s.md")).unwrap();
        assert!(matches!(v.resolve("link/secret.md"), Err(VaultError::Escape)));
        assert!(matches!(v.resolve("s.md"), Err(VaultError::Escape)));
        let mut v = v;
        v.rescan().unwrap();
        assert_eq!(v.count().unwrap(), 0, "symlinks are never followed");
        assert_eq!(fs::read_to_string(outside.join("secret.md")).unwrap(), "secret");
        assert!(v.resolve("ok.md").is_ok());
    }
}

#[test]
fn relative_vault_root_is_refused() {
    let t = tempfile::tempdir().unwrap();
    assert!(Vault::open(Path::new("rel/notes"), &t.path().join("i.db")).is_err());
}

#[test]
fn index_is_derived_rebuild_and_corruption() {
    let t = tempfile::tempdir().unwrap();
    let idx = t.path().join("index").join("vault.db");
    let id = {
        let mut v = open(t.path());
        let d = v.create(Some("Keep"), Some("m1"), None).unwrap();
        assert_eq!(v.rebuild().unwrap(), 1);
        d.id
    };
    // Index deleted: rebuilt from the files with the same ids and meeting links.
    fs::remove_file(&idx).unwrap();
    let _ = fs::remove_file(idx.with_extension("db-wal"));
    let v = open(t.path());
    assert_eq!(v.list(None, Some("m1"), 10).unwrap()[0].id, id);
    drop(v);
    // Index corrupted: replaced, nothing lost.
    fs::write(&idx, b"this is not sqlite at all, not even close......").unwrap();
    let _ = fs::remove_file(idx.with_extension("db-wal"));
    let _ = fs::remove_file(idx.with_extension("db-shm"));
    let v = open(t.path());
    assert_eq!(v.count().unwrap(), 1);
}

#[test]
fn meeting_links_trash_and_hud_append() {
    let t = tempfile::tempdir().unwrap();
    let mut v = open(t.path());
    let d = v.create(Some("Standalone"), None, None).unwrap();
    assert!(d.meetings.is_empty());
    let d = v.link_meeting(&d.id, "m9").unwrap();
    assert_eq!(d.meetings, vec!["m9"]);
    assert_eq!(v.list(None, Some("m9"), 10).unwrap().len(), 1);
    let d = v.unlink_meeting(&d.id, "m9").unwrap();
    assert!(d.meetings.is_empty());
    // HUD quick add: creates the meeting's note once, then appends (text only, no timestamp).
    let a = v.append_to_meeting("m2", "Standup", "ship it").unwrap();
    let b = v.append_to_meeting("m2", "Standup", "  and test it ").unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(b.body, "# Standup\n\n- ship it\n- and test it\n");
    // The HUD's note blocks a later migration of the same meeting's old rows (they hold the same lines).
    let legacy = [LegacyMeeting {
        id: "m2".into(),
        title: "Standup".into(),
        lines: vec![(0, "ship it".into())],
    }];
    assert!(migrate_meeting_notes(&mut v, &legacy).unwrap().is_empty());
    v.trash(&d.id).unwrap();
    assert!(t.path().join("notes/.trash/Standalone.md").exists());
    assert_eq!(v.count().unwrap(), 1);
}

#[test]
fn migration_is_lossless_and_idempotent() {
    let t = tempfile::tempdir().unwrap();
    let ms = vec![
        LegacyMeeting {
            id: "m1".into(),
            title: "Sync / weekly".into(),
            lines: vec![(65_000, "first".into()), (3_700_000, "two\nlines".into())],
        },
        LegacyMeeting {
            id: "m2".into(),
            title: "No notes".into(),
            lines: vec![],
        },
        LegacyMeeting {
            id: "m3".into(),
            title: "Sync / weekly".into(),
            lines: vec![(0, "ünïcode ✓".into())],
        },
    ];
    let mut v = open(t.path());
    assert_eq!(migrate_meeting_notes(&mut v, &ms).unwrap(), vec!["m1", "m3"]);
    let n1 = &v.list(None, Some("m1"), 10).unwrap()[0];
    let d1 = v.get(&n1.id).unwrap();
    assert_eq!(
        d1.body,
        "# Sync / weekly\n\n- [1:05] first\n- [1:01:40] two\n  lines\n"
    );
    assert_eq!(
        v.list(None, Some("m2"), 10).unwrap(),
        vec![],
        "a meeting without notes makes no note"
    );
    assert!(v
        .get(&v.list(None, Some("m3"), 10).unwrap()[0].id)
        .unwrap()
        .body
        .contains("ünïcode ✓"));
    // Again, and again after the index is lost: nothing new.
    assert_eq!(migrate_meeting_notes(&mut v, &ms).unwrap(), Vec::<String>::new());
    drop(v);
    fs::remove_dir_all(t.path().join("index")).unwrap();
    let mut v = open(t.path());
    assert_eq!(migrate_meeting_notes(&mut v, &ms).unwrap(), Vec::<String>::new());
    assert_eq!(v.count().unwrap(), 2);
}

/// Kill during save: a child process saves a big note in a loop and is SIGKILLed; the file is always one whole
/// version (never torn), and leftover temp files are swept and never indexed.
#[cfg(unix)]
#[test]
fn kill_during_save_never_tears_a_note() {
    if let Ok(dir) = std::env::var("QRTZ_VAULT_CHILD") {
        let mut v = Vault::open(
            &Path::new(&dir).join("notes"),
            &Path::new(&dir).join("index/vault.db"),
        )
        .unwrap();
        let id = v.list(None, None, 1).unwrap()[0].id.clone();
        let mut d = v.get(&id).unwrap();
        for i in 0u64.. {
            let c = (b'a' + (i % 26) as u8) as char;
            let body = format!("# Big\n{}\n", c.to_string().repeat(400_000));
            d = v.save(&id, &body, &d.hash, false).unwrap();
        }
        return;
    }
    let t = tempfile::tempdir().unwrap();
    {
        let mut v = open(t.path());
        let d = v.create(Some("Big"), None, None).unwrap();
        v.save(
            &d.id,
            &format!("# Big\n{}\n", "a".repeat(400_000)),
            &d.hash,
            false,
        )
        .unwrap();
    }
    for round in 0..8 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "kill_during_save_never_tears_a_note", "--nocapture"])
            .env("QRTZ_VAULT_CHILD", t.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150 + 37 * round));
        unsafe_kill(child.id());
        let _ = child.wait();
        let raw = fs::read_to_string(t.path().join("notes/Big.md")).unwrap();
        let n = parse_note(&raw);
        let line = n.body.lines().nth(1).unwrap();
        assert_eq!(line.len(), 400_000, "round {round}: torn file");
        let c = line.chars().next().unwrap();
        assert!(line.chars().all(|x| x == c), "round {round}: mixed versions");
        let mut v = open(t.path());
        assert_eq!(v.count().unwrap(), 1, "temp files are never indexed");
        v.rescan().unwrap();
        let temps = fs::read_dir(t.path().join("notes"))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("qrtz-tmp")
            })
            .count();
        assert_eq!(temps, 0, "temp files are swept");
    }
}

#[cfg(unix)]
fn unsafe_kill(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}
