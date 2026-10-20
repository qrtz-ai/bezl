//! QRT-286: the legacy sweep deletes only untagged `ai.qrtz.dash-net` owner-key items, and a second run is a no-op.
//! The fake deletes like the real Keychain: by service, account and creation date, whatever the comment.
use qrtz_dash_net::legacy_sweep::*;
use qrtz_dash_net::*;
use std::cell::RefCell;
use std::sync::Mutex;

#[derive(Default)]
struct Fake {
    items: Mutex<Vec<ItemAttrs>>,
    deletes: Mutex<usize>,
    fail_delete: bool,
    listed: RefCell<Vec<String>>,
}

impl SweepKeychain for Fake {
    fn list(&self, service: &str) -> Result<Vec<ItemAttrs>, String> {
        self.listed.borrow_mut().push(service.into());
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|i| i.service == service)
            .cloned()
            .collect())
    }
    fn delete(&self, service: &str, account: &str, created: &str) -> Result<(), String> {
        if self.fail_delete {
            return Err("errSecInteractionNotAllowed".into());
        }
        *self.deletes.lock().unwrap() += 1;
        self.items
            .lock()
            .unwrap()
            .retain(|i| !(i.service == service && i.account == account && i.created == created));
        Ok(())
    }
}

fn item(service: &str, account: &str, comment: Option<&str>, created: &str) -> ItemAttrs {
    ItemAttrs {
        service: service.into(),
        account: account.into(),
        comment: comment.map(Into::into),
        created: created.into(),
    }
}

const T1: &str = "2026-09-01 10:00:00 +0000";
const T2: &str = "2026-10-20 21:03:06 +0000";
const TAG: &str = "qrtz-dash origin https://dash.qrtz.ai";

fn world() -> Vec<ItemAttrs> {
    vec![
        item(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, None, T1),
        item(KEYCHAIN_SERVICE, KEYCHAIN_PENDING_ACCOUNT, Some(""), T1),
        item(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, Some(TAG), T2),
        item(KEYCHAIN_SERVICE, "someone-else", None, T1),
        item(DEV_KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, None, T1),
        item(
            DEV_KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
            Some("qrtz-dash origin http://127.0.0.1:8787"),
            T2,
        ),
        item("com.example.other", KEYCHAIN_ACCOUNT, None, T1),
    ]
}

#[test]
fn deletes_only_untagged_dash_net_owner_key_items() {
    let k = Fake {
        items: Mutex::new(world()),
        ..Default::default()
    };
    let r = sweep_untagged(&k).unwrap();
    assert_eq!(
        r,
        SweepReport {
            deleted: 2,
            skipped: 0
        }
    );
    let left = k.items.lock().unwrap().clone();
    let mut want = world();
    want.retain(|i| {
        !(i.service == KEYCHAIN_SERVICE && i.created == T1 && i.account.starts_with("owner-key"))
    });
    assert_eq!(left, want);
    // Tagged dash-net, every qrtz-dash-dev item, other services and other accounts all survive.
    assert!(left.contains(&item(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, Some(TAG), T2)));
    assert_eq!(
        left.iter().filter(|i| i.service == DEV_KEYCHAIN_SERVICE).count(),
        2
    );
    assert!(left.iter().any(|i| i.service == "com.example.other"));
    assert!(left.iter().any(|i| i.account == "someone-else"));
    // Only the real service is ever listed, never the dev one.
    assert_eq!(*k.listed.borrow(), vec![KEYCHAIN_SERVICE.to_string()]);
}

#[test]
fn a_second_run_changes_nothing() {
    let k = Fake {
        items: Mutex::new(world()),
        ..Default::default()
    };
    sweep_untagged(&k).unwrap();
    let after_first = k.items.lock().unwrap().clone();
    let deletes = *k.deletes.lock().unwrap();
    assert_eq!(sweep_untagged(&k).unwrap(), SweepReport::default());
    assert_eq!(*k.items.lock().unwrap(), after_first);
    assert_eq!(
        *k.deletes.lock().unwrap(),
        deletes,
        "no delete call on the second run"
    );
}

#[test]
fn an_untagged_item_sharing_a_tagged_items_creation_date_is_left_alone() {
    // The delete matches service, account and date, so it would take the tagged item too: skip it.
    let items = vec![
        item(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, None, T2),
        item(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, Some(TAG), T2),
    ];
    let k = Fake {
        items: Mutex::new(items.clone()),
        ..Default::default()
    };
    assert_eq!(
        sweep_untagged(&k).unwrap(),
        SweepReport {
            deleted: 0,
            skipped: 1
        }
    );
    assert_eq!(*k.items.lock().unwrap(), items);
    assert_eq!(*k.deletes.lock().unwrap(), 0);
}

#[test]
fn stops_at_the_first_error() {
    let k = Fake {
        items: Mutex::new(world()),
        fail_delete: true,
        ..Default::default()
    };
    assert!(sweep_untagged(&k).is_err());
    assert_eq!(*k.items.lock().unwrap(), world());
}

#[test]
fn nothing_to_sweep_is_ok() {
    let k = Fake::default();
    assert_eq!(sweep_untagged(&k).unwrap(), SweepReport::default());
}
