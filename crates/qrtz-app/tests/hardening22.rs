//! Cycle 22: the owner-only atomic write behind secrets and config (N30, N33, N52, T4). Its TS tests were deleted
//! with Electron (apps/desktop/src/main/secret-store.hardening6.test.ts, talk-service.hardening5.test.ts); before
//! this file the Rust side only checked the mode of a freshly written `secrets.json`.
#![cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use qrtz_app::secrets::{write_private_atomic, FileSecretStore, SecretStore};

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// A 0644 file an older build wrote, and a 0644 `.tmp` a crash left next to it, both end up as one 0600 file with
/// the new bytes; the stale `.tmp` is never reused (its mode would survive) and is gone afterwards.
#[test]
fn an_older_0644_file_and_a_crash_left_0644_tmp_end_up_0600() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("secrets.json");
    let tmp = dir.path().join("secrets.json.tmp");
    std::fs::write(&f, b"{\"test.a\":\"old\"}").unwrap();
    std::fs::write(&tmp, b"{\"half").unwrap();
    for p in [&f, &tmp] {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    write_private_atomic(&f, b"{\"test.a\":\"new\"}").unwrap();
    assert_eq!(std::fs::read(&f).unwrap(), b"{\"test.a\":\"new\"}");
    assert_eq!(mode(&f), 0o600);
    assert!(!tmp.exists());
}

/// A missing parent directory is created owner-only instead of failing the first save.
#[test]
fn a_missing_parent_is_created_0700() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a").join("config.json");
    write_private_atomic(&f, b"{}").unwrap();
    assert_eq!(mode(&f), 0o600);
    assert_eq!(mode(f.parent().unwrap()), 0o700);
}

/// A write that cannot complete (here the `.tmp` cannot be replaced, as on a full or read-only disk) returns the
/// error and leaves every saved key as it was; once the obstacle is gone the next write succeeds.
#[test]
fn a_failed_write_keeps_every_saved_key_and_the_next_one_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("secrets.json");
    let store = FileSecretStore::new(f.clone());
    store.set("test.a", Some("sk-or-1")).unwrap();
    store.set("test.b", Some("el-2")).unwrap();
    let blocker = dir.path().join("secrets.json.tmp");
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(blocker.join("x"), b"x").unwrap();
    assert!(store.set("test.c", Some("sk-3")).is_err());
    assert_eq!(store.list(), ["test.a", "test.b"]);
    assert_eq!(store.get("test.b").as_deref(), Some("el-2"));
    std::fs::remove_dir_all(&blocker).unwrap();
    store.set("test.c", Some("sk-3")).unwrap();
    assert_eq!(store.list(), ["test.a", "test.b", "test.c"]);
    assert_eq!(mode(&f), 0o600);
}

/// N106: a corrupt (or not owner-readable) `secrets.json` is not an empty store. A save fails with the reason and
/// leaves the file byte for byte, so the keys in it aren't dropped by the next write; reads stay empty meanwhile.
/// A missing file is still an empty store.
#[test]
fn a_corrupt_secrets_file_is_an_error_and_a_save_never_overwrites_it() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("secrets.json");
    let store = FileSecretStore::new(f.clone());
    assert!(store.list().is_empty());
    let corrupt = b"{\"test.a\":\"keep-me\",\"test.b\":";
    std::fs::write(&f, corrupt).unwrap();
    let e = store.set("test.c", Some("new")).unwrap_err();
    assert!(e.message.contains("corrupt"), "{}", e.message);
    assert_eq!(std::fs::read(&f).unwrap(), corrupt);
    assert!(store.list().is_empty());
    assert_eq!(store.get("test.a"), None);
    assert!(
        store.set("test.a", None).is_err(),
        "a removal doesn't rewrite it either"
    );
    assert_eq!(std::fs::read(&f).unwrap(), corrupt);
    qrtz_app::secrets::drop_retired(&store);
    assert_eq!(std::fs::read(&f).unwrap(), corrupt);

    std::fs::write(&f, br#"{"test.a":"keep-me"}"#).unwrap();
    store.set("test.c", Some("new")).unwrap();
    assert_eq!(store.list(), ["test.a", "test.c"]);
}
