#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
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
