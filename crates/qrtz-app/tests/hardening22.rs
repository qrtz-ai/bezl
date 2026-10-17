//! Cycle 22: the owner-only atomic write behind the config (N30, N33, N52, T4), and (ADR 0078, which removed the
//! secret store) the launch cleanup of keys an older build saved. Its TS tests were deleted
//! with Electron (apps/desktop/src/main/secret-store.hardening6.test.ts, talk-service.hardening5.test.ts); before
//! this file the Rust side only checked the mode of a freshly written `secrets.json`.
#![cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use qrtz_app::secrets::{drop_saved_keys, write_private_atomic, Dropped, Keychain, KEYCHAIN_SERVICE};
use std::sync::Mutex;

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
/// error and leaves the file as it was; once the obstacle is gone the next write succeeds.
#[test]
fn a_failed_write_keeps_the_file_and_the_next_one_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("config.json");
    write_private_atomic(&f, b"{\"theme\":\"light\"}").unwrap();
    let blocker = dir.path().join("config.json.tmp");
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(blocker.join("x"), b"x").unwrap();
    assert!(write_private_atomic(&f, b"{}").is_err());
    assert_eq!(std::fs::read(&f).unwrap(), b"{\"theme\":\"light\"}");
    std::fs::remove_dir_all(&blocker).unwrap();
    write_private_atomic(&f, b"{}").unwrap();
    assert_eq!(std::fs::read(&f).unwrap(), b"{}");
    assert_eq!(mode(&f), 0o600);
}

/// A fake Keychain: records deletions; `fail` names an item it can't delete.
#[derive(Default)]
struct FakeKeychain {
    deleted: Mutex<Vec<(String, String)>>,
    fail: Option<&'static str>,
}

impl Keychain for FakeKeychain {
    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        if self.fail == Some(account) {
            return Err("denied".into());
        }
        self.deleted
            .lock()
            .unwrap()
            .push((service.into(), account.into()));
        Ok(())
    }
}

/// ADR 0078: every key an older build saved is deleted at launch, whatever its name: the Keychain items its index
/// lists (then the index), and `secrets.json` even when corrupt (N106's file: nothing reads it any more).
#[test]
fn every_saved_key_is_deleted_at_launch_and_a_corrupt_file_too() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("secrets-index.json"), br#"["llm.openrouter","test.a"]"#).unwrap();
    std::fs::write(d.join("secrets.json"), b"{\"test.a\":\"keep-me\",\"test.b\":").unwrap();
    std::fs::write(d.join("secrets.json.tmp"), b"{").unwrap();
    let kc = FakeKeychain::default();
    let out = drop_saved_keys(d, &kc);
    assert_eq!(
        out,
        Dropped {
            keychain: vec!["llm.openrouter".into(), "test.a".into()],
            file: true
        }
    );
    assert_eq!(
        *kc.deleted.lock().unwrap(),
        [
            (KEYCHAIN_SERVICE.to_owned(), "llm.openrouter".to_owned()),
            (KEYCHAIN_SERVICE.to_owned(), "test.a".to_owned())
        ]
    );
    for f in ["secrets-index.json", "secrets.json", "secrets.json.tmp"] {
        assert!(!d.join(f).exists(), "{f}");
    }
    // Nothing left: the next launch does nothing (no Keychain call).
    let kc = FakeKeychain::default();
    assert_eq!(drop_saved_keys(d, &kc), Dropped::default());
    assert!(kc.deleted.lock().unwrap().is_empty());
}

/// A Keychain item that can't be deleted keeps the index, so the next launch tries again; the others still go.
#[test]
fn a_keychain_item_that_cannot_be_deleted_is_retried_next_launch() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("secrets-index.json"), br#"["a","b"]"#).unwrap();
    let kc = FakeKeychain {
        fail: Some("a"),
        ..Default::default()
    };
    assert_eq!(drop_saved_keys(d, &kc).keychain, ["b"]);
    assert!(d.join("secrets-index.json").exists());
    let kc = FakeKeychain::default();
    assert_eq!(drop_saved_keys(d, &kc).keychain, ["a", "b"]);
    assert!(!d.join("secrets-index.json").exists());
}
