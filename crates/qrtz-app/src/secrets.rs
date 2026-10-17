//! qrtz keeps no secrets (ADR 0076, ADR 0078): it is entirely offline, so no feature has an API key. There is no
//! secret store, no `secrets:*` channel and no `QRTZ_SECRET_*` reader. What is left:
//! - [`drop_saved_keys`], the launch cleanup of keys an older build saved: the macOS Keychain items listed in
//!   `secrets-index.json` (service `ai.qrtz.desktop`) and the owner-only `secrets.json` of non-macOS builds.
//! - [`write_private_atomic`], the 0600 atomic write the config store uses (N30/N33/T4).
use std::path::Path;

/// The Keychain service older builds saved keys under.
pub const KEYCHAIN_SERVICE: &str = "ai.qrtz.desktop";
/// Older macOS builds: the names-only index of their Keychain items.
pub const KEYCHAIN_INDEX: &str = "secrets-index.json";
/// Older non-macOS builds: `{name: value}`, 0600.
pub const SECRETS_FILE: &str = "secrets.json";

/// Deletes one generic password. Not-found is success (already gone).
pub trait Keychain: Send + Sync {
    fn delete(&self, service: &str, account: &str) -> Result<(), String>;
}

/// The login Keychain, from this process (an item qrtz created needs no prompt to delete).
#[cfg(target_os = "macos")]
pub struct SystemKeychain;

#[cfg(target_os = "macos")]
impl Keychain for SystemKeychain {
    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
        match security_framework::passwords::delete_generic_password(service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// No Keychain on this platform (older builds here used [`SECRETS_FILE`]).
pub struct NoKeychain;

impl Keychain for NoKeychain {
    fn delete(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// This platform's Keychain for [`drop_saved_keys`].
pub fn platform_keychain() -> Box<dyn Keychain> {
    #[cfg(target_os = "macos")]
    {
        Box::new(SystemKeychain)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoKeychain)
    }
}

/// What [`drop_saved_keys`] removed (names only, never values).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    pub keychain: Vec<String>,
    pub file: bool,
}

/// Launch cleanup (ADR 0078): delete every key an older build saved, whatever its name, since nothing reads keys.
/// The Keychain index is removed once all of its items are gone (a failure keeps it, so the next launch retries);
/// `secrets.json` (and a crash-left `.tmp`) is removed whatever it holds, also when it is corrupt. Cheap when
/// there is nothing to do: two missing files, no Keychain call.
pub fn drop_saved_keys(data: &Path, kc: &dyn Keychain) -> Dropped {
    let mut out = Dropped::default();
    let index = data.join(KEYCHAIN_INDEX);
    if let Ok(text) = std::fs::read_to_string(&index) {
        let names: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
        let mut failed = false;
        for n in &names {
            match kc.delete(KEYCHAIN_SERVICE, n) {
                Ok(()) => out.keychain.push(n.clone()),
                Err(e) => {
                    failed = true;
                    tracing::warn!(name = %n, err = %e, "couldn't delete a saved key; retrying next launch");
                }
            }
        }
        if !failed {
            remove(&index);
        }
    }
    let file = data.join(SECRETS_FILE);
    if file.exists() {
        out.file = remove(&file);
    }
    let mut tmp = file.into_os_string();
    tmp.push(".tmp");
    remove(Path::new(&tmp));
    if !out.keychain.is_empty() || out.file {
        tracing::info!(
            keychain = out.keychain.len(),
            file = out.file,
            "saved keys from an older build deleted"
        );
    }
    out
}

fn remove(p: &Path) -> bool {
    match std::fs::remove_file(p) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            tracing::warn!(path = %p.display(), err = %e, "couldn't delete");
            false
        }
    }
}

/// Write `bytes` to `file` atomically and owner-only: remove a stale `<file>.tmp` (an existing file keeps its old
/// mode), create the tmp 0600, write, fsync, rename over the target.
pub fn write_private_atomic(file: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = file.parent() {
        crate::dirs::private_dir(dir)?;
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, file)
}
