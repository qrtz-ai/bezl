//! QRT-286: the startup sweep for owner-key items saved before origin tagging (f9c8518).
//!
//! Before f9c8518 a dev build saved the owner key under [`KEYCHAIN_SERVICE`] with no comment. Every lookup now
//! includes the origin comment ([`crate::key_origin_tag`]), so such an item can't be read or deleted by the store,
//! and in the login keychain it would block saving a tagged item under the same account.
//!
//! The sweep, run once at every helper start:
//! - looks only at [`KEYCHAIN_SERVICE`] (never [`crate::DEV_KEYCHAIN_SERVICE`] or any other service) and only at
//!   the [`KEYCHAIN_ACCOUNT`] and [`KEYCHAIN_PENDING_ACCOUNT`] accounts;
//! - lists items by attributes only: the secret is never requested or read;
//! - deletes an item only if it has no comment and it is the only item listed under its account (the delete
//!   matches service and account, so it runs only when that query can't reach a tagged item), then lists again and
//!   fails if the untagged item is still there or any tagged item is gone;
//! - is idempotent: once the untagged items are gone a run finds nothing and changes nothing;
//! - is bounded: one listing, at most one delete per listed item, and it stops at the first unexpected error.
use crate::{KEYCHAIN_ACCOUNT, KEYCHAIN_PENDING_ACCOUNT, KEYCHAIN_SERVICE};

/// One listed item: attributes only, never the secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemAttrs {
    pub service: String,
    pub account: String,
    /// The item's comment; `None` (or empty) means untagged.
    pub comment: Option<String>,
    /// The item's creation date, as the keychain reports it (second precision).
    pub created: String,
}

impl ItemAttrs {
    fn tagged(&self) -> bool {
        self.comment.as_deref().is_some_and(|c| !c.is_empty())
    }
}

/// What the sweep needs from a keychain: an attribute-only listing and a delete by service and account. The macOS
/// Keychain in the helper, a fake in tests.
pub trait SweepKeychain {
    /// Every generic-password item of `service`, attributes only. No items is `Ok(vec![])`.
    fn list(&self, service: &str) -> Result<Vec<ItemAttrs>, String>;
    /// Deletes every item of `service`/`account`, whatever its comment. Nothing to delete is `Ok(())`.
    fn delete(&self, service: &str, account: &str) -> Result<(), String>;
}

/// What a sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Untagged items deleted.
    pub deleted: usize,
    /// Untagged items left because a tagged item shares their account.
    pub skipped: usize,
}

/// Deletes the untagged legacy owner-key items (see the module docs). Never touches a tagged item, another
/// service, or another account; stops at the first error.
pub fn sweep_untagged(k: &dyn SweepKeychain) -> Result<SweepReport, String> {
    let items = k.list(KEYCHAIN_SERVICE)?;
    let tagged: Vec<&ItemAttrs> = items.iter().filter(|i| i.tagged()).collect();
    let mut report = SweepReport::default();
    for account in [KEYCHAIN_ACCOUNT, KEYCHAIN_PENDING_ACCOUNT] {
        let under: Vec<&ItemAttrs> = items
            .iter()
            .filter(|i| i.service == KEYCHAIN_SERVICE && i.account == account)
            .collect();
        let untagged = under.iter().filter(|i| !i.tagged()).count();
        if untagged == 0 {
            continue;
        }
        if untagged != under.len() {
            report.skipped += untagged;
            continue;
        }
        k.delete(KEYCHAIN_SERVICE, account)?;
        let now = k.list(KEYCHAIN_SERVICE)?;
        if now.iter().any(|i| i.account == account && !i.tagged()) {
            return Err(format!(
                "legacy keychain sweep: the untagged {account} item is still there"
            ));
        }
        if tagged.iter().any(|t| !now.contains(t)) {
            return Err("legacy keychain sweep: a tagged item went missing".into());
        }
        report.deleted += untagged;
    }
    Ok(report)
}

/// The macOS Keychain the store uses (login keychain in debug builds; the data-protection keychain and
/// [`crate::ACCESS_GROUP`] in release builds).
#[cfg(target_os = "macos")]
pub struct KeychainSweep;

#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
impl SweepKeychain for KeychainSweep {
    fn list(&self, service: &str) -> Result<Vec<ItemAttrs>, String> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
        let mut q = ItemSearchOptions::new();
        q.class(ItemClass::generic_password())
            .service(service)
            .load_attributes(true)
            .load_data(false)
            .load_refs(false)
            .limit(Limit::All);
        if cfg!(not(debug_assertions)) {
            q.ignore_legacy_keychains().access_group(crate::ACCESS_GROUP);
        }
        let found = match q.search() {
            Ok(r) => r,
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(Vec::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut out = Vec::new();
        for r in found {
            let a = r
                .simplify_dict()
                .ok_or("keychain listing returned a non-attribute result")?;
            out.push(ItemAttrs {
                service: a.get("svce").cloned().unwrap_or_default(),
                account: a.get("acct").cloned().unwrap_or_default(),
                comment: a.get("icmt").cloned(),
                created: a
                    .get("cdat")
                    .cloned()
                    .ok_or("keychain item without a creation date")?,
            });
        }
        Ok(out)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        use security_framework::passwords::{delete_generic_password_options, PasswordOptions};
        let mut o = PasswordOptions::new_generic_password(service, account);
        if cfg!(not(debug_assertions)) {
            o.use_protected_keychain();
            o.set_access_group(crate::ACCESS_GROUP);
        }
        match delete_generic_password_options(o) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The startup sweep against the platform keychain. No Keychain: nothing to sweep.
pub fn platform_sweep() -> Result<SweepReport, String> {
    #[cfg(target_os = "macos")]
    {
        sweep_untagged(&KeychainSweep)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(SweepReport::default())
    }
}
