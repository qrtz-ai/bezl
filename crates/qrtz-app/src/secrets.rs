//! Secrets (LLM / ASR API keys). macOS: Keychain via `keyring`, with a names-only index file (values never
//! touch disk outside the Keychain). Elsewhere (Linux dev/test builds, not a ship target): an owner-only JSON
//! file. Every file is written atomically at 0600 (N30/N33/T4: a crash-left `.tmp` is removed first, then
//! rename). `QRTZ_SECRET_*` env vars override (CI, headless), like Electron's `envNamesFor`.
use qrtz_ipc::AppError;
#[cfg(target_os = "macos")]
use qrtz_ipc::ErrorCode;
use std::collections::BTreeMap;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn set(&self, name: &str, value: Option<&str>) -> Result<(), AppError>;
    fn get(&self, name: &str) -> Option<String>;
    fn list(&self) -> Vec<String>;
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

/// `QRTZ_SECRET_LLM_OPENAI` etc. (and the short `QRTZ_SECRET_OPENROUTER` for `llm.*`).
pub fn env_names_for(name: &str) -> Vec<String> {
    let f = |n: &str| {
        "QRTZ_SECRET_".to_string()
            + &n.to_uppercase()
                .chars()
                .map(|c| {
                    if c.is_ascii_uppercase() || c.is_ascii_digit() {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
    };
    let mut v = vec![f(name)];
    if let Some(short) = name.strip_prefix("llm.") {
        v.push(f(short));
    }
    v
}

/// Env overrides in front of any store (reads only; `set` goes to the store).
pub struct EnvOverride<S: SecretStore>(pub S);
impl<S: SecretStore> SecretStore for EnvOverride<S> {
    fn set(&self, name: &str, value: Option<&str>) -> Result<(), AppError> {
        self.0.set(name, value)
    }
    fn get(&self, name: &str) -> Option<String> {
        env_names_for(name)
            .iter()
            .find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
            .or_else(|| self.0.get(name))
    }
    fn list(&self) -> Vec<String> {
        let mut names: std::collections::BTreeSet<String> = self.0.list().into_iter().collect();
        for (k, _) in std::env::vars() {
            if k.starts_with("QRTZ_SECRET_") {
                names.insert(k);
            }
        }
        names.into_iter().collect()
    }
}

/// Non-macOS store: `{name: value}` JSON in an owner-only file (0600, atomic).
pub struct FileSecretStore {
    file: std::path::PathBuf,
    lock: Mutex<()>,
}
impl FileSecretStore {
    pub fn new(file: std::path::PathBuf) -> Self {
        Self {
            file,
            lock: Mutex::new(()),
        }
    }
    fn read(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(&self.file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
}
impl SecretStore for FileSecretStore {
    fn set(&self, name: &str, value: Option<&str>) -> Result<(), AppError> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut all = self.read();
        match value {
            Some(v) => all.insert(name.to_owned(), v.to_owned()),
            None => all.remove(name),
        };
        let bytes = serde_json::to_vec(&all).unwrap_or_default();
        write_private_atomic(&self.file, &bytes).map_err(|e| {
            AppError::new(
                qrtz_ipc::ErrorCode::Internal,
                format!("secret store write failed: {e}"),
            )
        })
    }
    fn get(&self, name: &str) -> Option<String> {
        self.read().get(name).cloned()
    }
    fn list(&self) -> Vec<String> {
        self.read().into_keys().collect()
    }
}

#[derive(Default)]
pub struct MemorySecretStore(Mutex<BTreeMap<String, String>>);

impl SecretStore for MemorySecretStore {
    fn set(&self, name: &str, value: Option<&str>) -> Result<(), AppError> {
        let mut m = self.0.lock().unwrap();
        match value {
            Some(v) => m.insert(name.to_owned(), v.to_owned()),
            None => m.remove(name),
        };
        Ok(())
    }
    fn get(&self, name: &str) -> Option<String> {
        self.0.lock().unwrap().get(name).cloned()
    }
    fn list(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
}

#[cfg(target_os = "macos")]
pub struct KeychainSecretStore {
    service: String,
    index: std::path::PathBuf,
    lock: Mutex<()>,
}

#[cfg(target_os = "macos")]
impl KeychainSecretStore {
    pub fn new(service: &str, index: std::path::PathBuf) -> Self {
        Self {
            service: service.into(),
            index,
            lock: Mutex::new(()),
        }
    }
    fn names(&self) -> Vec<String> {
        std::fs::read_to_string(&self.index)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
}

#[cfg(target_os = "macos")]
impl SecretStore for KeychainSecretStore {
    fn set(&self, name: &str, value: Option<&str>) -> Result<(), AppError> {
        let _g = self.lock.lock().unwrap();
        let entry = keyring::Entry::new(&self.service, name)
            .map_err(|e| AppError::new(ErrorCode::Internal, format!("keychain: {e}")))?;
        let mut names = self.names();
        match value {
            Some(v) => {
                entry
                    .set_password(v)
                    .map_err(|e| AppError::new(ErrorCode::Internal, format!("keychain: {e}")))?;
                if !names.iter().any(|n| n == name) {
                    names.push(name.into());
                }
            }
            None => {
                let _ = entry.delete_credential();
                names.retain(|n| n != name);
            }
        }
        names.sort();
        let _ = write_private_atomic(
            &self.index,
            serde_json::to_string(&names).unwrap_or_default().as_bytes(),
        );
        Ok(())
    }
    fn get(&self, name: &str) -> Option<String> {
        keyring::Entry::new(&self.service, name).ok()?.get_password().ok()
    }
    fn list(&self) -> Vec<String> {
        self.names()
    }
}
