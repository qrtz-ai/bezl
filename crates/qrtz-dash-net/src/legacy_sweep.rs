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
//! - deletes an item only if it has no comment, matched by service, account and creation date, and only when no
//!   tagged item shares that account and creation date (so the delete query can't match a tagged item);
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

/// What the sweep needs from a keychain: an attribute-only listing and a delete by service, account and creation
/// date. The macOS Keychain in the helper, a fake in tests.
pub trait SweepKeychain {
    /// Every generic-password item of `service`, attributes only. No items is `Ok(vec![])`.
    fn list(&self, service: &str) -> Result<Vec<ItemAttrs>, String>;
    /// Deletes the items of `service`/`account` created at `created`. Nothing to delete is `Ok(())`.
    fn delete(&self, service: &str, account: &str, created: &str) -> Result<(), String>;
}

/// What a sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Untagged items deleted.
    pub deleted: usize,
    /// Untagged items left because a tagged item shares their account and creation date.
    pub skipped: usize,
}

/// Deletes the untagged legacy owner-key items (see the module docs). Never touches a tagged item, another
/// service, or another account; stops at the first error.
pub fn sweep_untagged(k: &dyn SweepKeychain) -> Result<SweepReport, String> {
    let items = k.list(KEYCHAIN_SERVICE)?;
    let mut report = SweepReport::default();
    let mut done: Vec<(&str, &str)> = Vec::new();
    for it in &items {
        if it.service != KEYCHAIN_SERVICE
            || it.tagged()
            || !(it.account == KEYCHAIN_ACCOUNT || it.account == KEYCHAIN_PENDING_ACCOUNT)
        {
            continue;
        }
        let key = (it.account.as_str(), it.created.as_str());
        if done.contains(&key) {
            continue;
        }
        done.push(key);
        let clash = items.iter().any(|o| {
            o.tagged() && o.service == it.service && o.account == it.account && o.created == it.created
        });
        if clash {
            report.skipped += 1;
            continue;
        }
        k.delete(KEYCHAIN_SERVICE, &it.account, &it.created)?;
        report.deleted += 1;
    }
    Ok(report)
}

/// The macOS Keychain the store uses (login keychain in debug builds; the data-protection keychain and
/// [`crate::ACCESS_GROUP`] in release builds).
#[cfg(target_os = "macos")]
pub struct KeychainSweep;

#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Parses the keychain's date description, `YYYY-MM-DD hh:mm:ss +0000`, into Core Foundation absolute time.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn cf_absolute_time(created: &str) -> Option<f64> {
    let s = created.trim();
    let (date, rest) = s.split_once(' ')?;
    let (time, zone) = rest.split_once(' ')?;
    if zone != "+0000" {
        return None;
    }
    let d: Vec<i64> = date.split('-').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let t: Vec<i64> = time.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [y, m, dd]: [i64; 3] = d.try_into().ok()?;
    let [hh, mm, ss]: [i64; 3] = t.try_into().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&dd) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let unix = days * 86_400 + hh * 3600 + mm * 60 + ss;
    Some((unix - 978_307_200) as f64)
}

#[cfg(test)]
pub(crate) fn cf_absolute_time_for_tests(created: &str) -> Option<f64> {
    cf_absolute_time(created)
}

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

    fn delete(&self, service: &str, account: &str, created: &str) -> Result<(), String> {
        use core_foundation::base::TCFType;
        use core_foundation::date::CFDate;
        use core_foundation::string::CFString;
        use security_framework::passwords::{delete_generic_password_options, PasswordOptions};
        let at = cf_absolute_time(created).ok_or("unrecognised keychain creation date")?;
        let mut o = PasswordOptions::new_generic_password(service, account);
        if cfg!(not(debug_assertions)) {
            o.use_protected_keychain();
            o.set_access_group(crate::ACCESS_GROUP);
        }
        // kSecAttrCreationDate ("cdat"): security-framework has no setter for it, and its constant needs unsafe.
        #[allow(deprecated)]
        o.query
            .push((CFString::from_static_string("cdat"), CFDate::new(at).as_CFType()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_keychain_date_description() {
        assert_eq!(cf_absolute_time_for_tests("2001-01-01 00:00:00 +0000"), Some(0.0));
        assert_eq!(
            cf_absolute_time_for_tests("2026-10-20 21:03:06 +0000"),
            Some(814_222_986.0)
        );
        assert_eq!(cf_absolute_time_for_tests("2026-10-20 21:03:06 -0700"), None);
        assert_eq!(cf_absolute_time_for_tests("nonsense"), None);
    }
}
