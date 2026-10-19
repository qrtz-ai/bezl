//! Bozniak M6: saving a new owner key never leaves the helper without a readable key, whatever step fails.
use qrtz_dash_net::*;
use std::collections::HashMap;
use std::sync::Mutex;

const OLD: &str =
    "ok_0123456789abcdef0123456789abcdef_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const NEW: &str =
    "ok_fedcba9876543210fedcba9876543210_fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

/// An in-memory keychain whose add/delete/get on an account can be made to fail, or `get` to return other bytes.
#[derive(Default)]
struct Faulty {
    items: Mutex<HashMap<String, Vec<u8>>>,
    fail_add: Mutex<Vec<String>>,
    fail_delete: Mutex<Vec<String>>,
    garble_get: Mutex<Vec<String>>,
    adds: Mutex<usize>,
}

impl RawKeychain for Faulty {
    fn get(&self, a: &str) -> Result<Option<Vec<u8>>, String> {
        let v = self.items.lock().unwrap().get(a).cloned();
        if self.garble_get.lock().unwrap().iter().any(|x| x == a) {
            return Ok(v.map(|_| b"garbled".to_vec()));
        }
        Ok(v)
    }
    fn add(&self, a: &str, s: &[u8]) -> Result<(), String> {
        *self.adds.lock().unwrap() += 1;
        if self.fail_add.lock().unwrap().iter().any(|x| x == a) {
            return Err("errSecInteractionNotAllowed".into());
        }
        let mut m = self.items.lock().unwrap();
        if m.contains_key(a) {
            return Err("errSecDuplicateItem".into());
        }
        m.insert(a.into(), s.to_vec());
        Ok(())
    }
    fn delete(&self, a: &str) -> Result<(), String> {
        if self.fail_delete.lock().unwrap().iter().any(|x| x == a) {
            return Err("errSecAuthFailed".into());
        }
        self.items.lock().unwrap().remove(a);
        Ok(())
    }
}

fn with_old() -> TwoPhase<Faulty> {
    let s = TwoPhase(Faulty::default());
    s.save(OLD).unwrap();
    s
}
fn accounts(s: &TwoPhase<Faulty>) -> Vec<String> {
    let mut v: Vec<_> = s.0.items.lock().unwrap().keys().cloned().collect();
    v.sort();
    v
}

#[test]
fn a_good_save_replaces_the_key_and_leaves_no_pending_item() {
    let s = with_old();
    s.save(NEW).unwrap();
    assert_eq!(s.load().as_deref(), Some(NEW));
    assert_eq!(accounts(&s), [KEYCHAIN_ACCOUNT]);
}

#[test]
fn a_failed_pending_add_keeps_the_old_key() {
    let s = with_old();
    s.0.fail_add.lock().unwrap().push(KEYCHAIN_PENDING_ACCOUNT.into());
    assert!(s.save(NEW).is_err());
    assert_eq!(s.load().as_deref(), Some(OLD));
}

#[test]
fn a_pending_key_that_does_not_read_back_keeps_the_old_key() {
    let s = with_old();
    s.0.garble_get
        .lock()
        .unwrap()
        .push(KEYCHAIN_PENDING_ACCOUNT.into());
    assert!(s.save(NEW).is_err());
    assert_eq!(s.load().as_deref(), Some(OLD));
    assert_eq!(accounts(&s), [KEYCHAIN_ACCOUNT]);
}

#[test]
fn a_failed_delete_of_the_old_item_keeps_the_old_key_readable() {
    let s = with_old();
    s.0.fail_delete.lock().unwrap().push(KEYCHAIN_ACCOUNT.into());
    let e = s.save(NEW).unwrap_err();
    assert!(e.contains("old key is kept"), "{e}");
    assert_eq!(s.load().as_deref(), Some(OLD));
    // The new key is not stranded under pending beside the old one.
    assert_eq!(accounts(&s), [KEYCHAIN_ACCOUNT]);
}

#[test]
fn a_failed_final_add_retries_then_keeps_the_new_key_under_pending() {
    let s = with_old();
    s.0.fail_add.lock().unwrap().push(KEYCHAIN_ACCOUNT.into());
    *s.0.adds.lock().unwrap() = 0;
    let e = s.save(NEW).unwrap_err();
    assert!(e.contains("kept"), "{e}");
    assert_eq!(*s.0.adds.lock().unwrap(), 1 + SAVE_ATTEMPTS);
    // The delete-before-save bug lost the key here; now it is still loadable.
    assert_eq!(s.load().as_deref(), Some(NEW));
    // The next save (keychain healthy again) moves it and cleans up.
    s.0.fail_add.lock().unwrap().clear();
    s.save(NEW).unwrap();
    assert_eq!(accounts(&s), [KEYCHAIN_ACCOUNT]);
    assert_eq!(s.load().as_deref(), Some(NEW));
}

#[test]
fn a_final_key_that_does_not_read_back_is_removed_and_the_pending_key_kept() {
    let s = with_old();
    s.0.garble_get.lock().unwrap().push(KEYCHAIN_ACCOUNT.into());
    assert!(s.save(NEW).is_err());
    assert_eq!(accounts(&s), [KEYCHAIN_PENDING_ACCOUNT]);
    assert_eq!(s.load().as_deref(), Some(NEW));
}

#[test]
fn a_stale_pending_item_does_not_block_a_save_and_disconnect_removes_both() {
    let s = with_old();
    s.0.items
        .lock()
        .unwrap()
        .insert(KEYCHAIN_PENDING_ACCOUNT.into(), b"stale".to_vec());
    s.save(NEW).unwrap();
    assert_eq!(accounts(&s), [KEYCHAIN_ACCOUNT]);
    s.0.items
        .lock()
        .unwrap()
        .insert(KEYCHAIN_PENDING_ACCOUNT.into(), NEW.as_bytes().to_vec());
    s.delete().unwrap();
    assert!(accounts(&s).is_empty());
    assert_eq!(s.load(), None);
}

#[test]
fn load_ignores_items_that_are_not_owner_keys() {
    let s = TwoPhase(Faulty::default());
    s.0.items
        .lock()
        .unwrap()
        .insert(KEYCHAIN_ACCOUNT.into(), b"nope".to_vec());
    assert_eq!(s.load(), None);
}

const THIRD: &str =
    "ok_00112233445566778899aabbccddeeff_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

#[test]
fn a_second_save_after_a_failed_final_add_never_deletes_the_only_key() {
    // Bozniak M6 round 2: save #1's final add fails, so the key is only under pending and main is empty.
    let s = with_old();
    s.0.fail_add.lock().unwrap().push(KEYCHAIN_ACCOUNT.into());
    assert!(s.save(NEW).is_err());
    assert_eq!(accounts(&s), [KEYCHAIN_PENDING_ACCOUNT]);
    // Save #2 with main still failing: it must not touch pending at all.
    s.0.fail_add.lock().unwrap().push(KEYCHAIN_PENDING_ACCOUNT.into());
    assert!(s.save(THIRD).is_err());
    assert_eq!(s.load().as_deref(), Some(NEW));
    // Save #2 with main healthy but its own pending add failing: the stranded key is promoted first.
    s.0.fail_add.lock().unwrap().retain(|a| a != KEYCHAIN_ACCOUNT);
    assert!(s.save(THIRD).is_err());
    assert_eq!(accounts(&s), [KEYCHAIN_ACCOUNT]);
    assert_eq!(s.load().as_deref(), Some(NEW));
}

#[test]
fn a_stranded_key_that_does_not_read_back_from_main_stays_under_pending() {
    let s = with_old();
    s.0.fail_add.lock().unwrap().push(KEYCHAIN_ACCOUNT.into());
    assert!(s.save(NEW).is_err());
    s.0.fail_add.lock().unwrap().clear();
    s.0.garble_get.lock().unwrap().push(KEYCHAIN_ACCOUNT.into());
    assert!(s.save(THIRD).is_err());
    s.0.garble_get.lock().unwrap().clear();
    assert_eq!(accounts(&s), [KEYCHAIN_PENDING_ACCOUNT]);
    assert_eq!(s.load().as_deref(), Some(NEW));
}
