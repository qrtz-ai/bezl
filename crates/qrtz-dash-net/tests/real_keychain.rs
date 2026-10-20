//! QRT-276: `TwoPhase(KeychainStore)` against a real macOS keychain (debug path: the user's default keychain).
//!
//! Ignored by default so `cargo test` never writes to a developer's login keychain. CI's `keychain` job creates a
//! throwaway keychain, makes it the default, sets QRTZ_TEST_KEYCHAIN to its path and runs `--ignored`. Each test
//! refuses to run unless the default keychain is exactly that path. Only test-generated keys are stored.
#![cfg(target_os = "macos")]
use qrtz_dash_net::*;
use std::hash::{BuildHasher, Hasher};
use std::process::Command;

fn hex(n: usize) -> String {
    let mut s = String::new();
    while s.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(s.len());
        s.push_str(&format!("{:016x}", h.finish()));
    }
    s.truncate(n);
    s
}
fn test_key() -> String {
    format!("ok_{}_{}", hex(32), hex(64))
}

fn store() -> TwoPhase<KeychainStore> {
    let want = std::env::var("QRTZ_TEST_KEYCHAIN")
        .expect("QRTZ_TEST_KEYCHAIN (a throwaway keychain path) is required");
    let out = Command::new("security").arg("default-keychain").output().unwrap();
    let got = String::from_utf8_lossy(&out.stdout);
    let got = got.trim().trim_matches('"');
    assert!(
        std::path::Path::new(got) == std::path::Path::new(&want),
        "default keychain is {got}, not the throwaway {want}; refusing to touch it"
    );
    assert!(
        !got.contains("login.keychain"),
        "refusing to use the login keychain"
    );
    TwoPhase(KeychainStore)
}

#[test]
#[ignore = "needs a throwaway default keychain (CI keychain job)"]
fn save_load_replace_and_delete_on_a_real_keychain() {
    let s = store();
    s.delete().unwrap();
    assert_eq!(s.load(), None);
    let (a, b) = (test_key(), test_key());
    assert!(is_owner_key(&a) && is_owner_key(&b) && a != b);
    s.save(&a).unwrap();
    assert_eq!(s.load().as_deref(), Some(a.as_str()));
    s.save(&b).unwrap();
    assert_eq!(s.load().as_deref(), Some(b.as_str()));
    assert_eq!(
        KeychainStore.get(KEYCHAIN_PENDING_ACCOUNT).unwrap(),
        None,
        "no pending item after a good save"
    );
    s.delete().unwrap();
    assert_eq!(s.load(), None);
    assert_eq!(KeychainStore.get(KEYCHAIN_ACCOUNT).unwrap(), None);
}

#[test]
#[ignore = "needs a throwaway default keychain (CI keychain job)"]
fn a_raw_add_on_an_existing_account_replaces_the_item() {
    // QRT-264: `RawKeychain::add` creates or updates (the Keychain adds, then updates on a duplicate). TwoPhase
    // deletes first, so it never depends on this; the test pins the documented behaviour.
    let s = store();
    s.delete().unwrap();
    let (a, b) = (test_key(), test_key());
    KeychainStore.add(KEYCHAIN_ACCOUNT, a.as_bytes()).unwrap();
    KeychainStore.add(KEYCHAIN_ACCOUNT, b.as_bytes()).unwrap();
    assert_eq!(
        KeychainStore.get(KEYCHAIN_ACCOUNT).unwrap().as_deref(),
        Some(b.as_bytes())
    );
    s.delete().unwrap();
}

/// An item with no comment, as a build from before f9c8518 saved it (test-generated value only).
fn add_untagged(service: &str, account: &str) {
    let o = security_framework::passwords::PasswordOptions::new_generic_password(service, account);
    security_framework::passwords::set_generic_password_options(test_key().as_bytes(), o).unwrap();
}
fn delete_any(service: &str, account: &str) {
    let _ = security_framework::passwords::delete_generic_password(service, account);
}
fn attrs(service: &str) -> Vec<legacy_sweep::ItemAttrs> {
    use legacy_sweep::SweepKeychain;
    let mut v = legacy_sweep::KeychainSweep.list(service).unwrap();
    v.sort_by(|a, b| a.account.cmp(&b.account));
    v
}

#[test]
#[ignore = "needs a throwaway default keychain (CI keychain job)"]
fn legacy_sweep_deletes_only_untagged_dash_net_items_and_is_idempotent() {
    // QRT-286: on a real keychain, the sweep removes the untagged ai.qrtz.dash-net item and nothing else.
    let s = store();
    const OTHER: &str = "ai.qrtz.test-other-service";
    s.delete().unwrap();
    for svc in [KEYCHAIN_SERVICE, DEV_KEYCHAIN_SERVICE, OTHER] {
        delete_any(svc, KEYCHAIN_ACCOUNT);
        delete_any(svc, KEYCHAIN_PENDING_ACCOUNT);
    }
    // Tagged dash-net item (the store's own add) and an untagged legacy one under the other account.
    let key = test_key();
    KeychainStore.add(KEYCHAIN_ACCOUNT, key.as_bytes()).unwrap();
    add_untagged(KEYCHAIN_SERVICE, KEYCHAIN_PENDING_ACCOUNT);
    add_untagged(DEV_KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT);
    add_untagged(OTHER, KEYCHAIN_ACCOUNT);
    let before = attrs(KEYCHAIN_SERVICE);
    assert_eq!(before.len(), 2, "{before:?}");

    let r = legacy_sweep::sweep_untagged(&legacy_sweep::KeychainSweep).unwrap();
    assert_eq!(
        r,
        legacy_sweep::SweepReport {
            deleted: 1,
            skipped: 0
        }
    );
    let after = attrs(KEYCHAIN_SERVICE);
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(after[0].account, KEYCHAIN_ACCOUNT);
    assert_eq!(after[0].comment.as_deref(), Some(key_origin_tag().as_str()));
    // The tagged key is still readable, unchanged.
    assert_eq!(s.load().as_deref(), Some(key.as_str()));
    assert_eq!(
        attrs(DEV_KEYCHAIN_SERVICE).len(),
        1,
        "qrtz-dash-dev item survives"
    );
    assert_eq!(attrs(OTHER).len(), 1, "other service's item survives");

    // Second run: nothing to do, nothing changes.
    let r = legacy_sweep::sweep_untagged(&legacy_sweep::KeychainSweep).unwrap();
    assert_eq!(r, legacy_sweep::SweepReport::default());
    assert_eq!(attrs(KEYCHAIN_SERVICE), after);
    assert_eq!(attrs(DEV_KEYCHAIN_SERVICE).len(), 1);
    assert_eq!(attrs(OTHER).len(), 1);

    s.delete().unwrap();
    delete_any(DEV_KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT);
    delete_any(OTHER, KEYCHAIN_ACCOUNT);
}
