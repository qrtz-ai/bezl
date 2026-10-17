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

/// Secret names no feature reads any more: `asr.deepgram` (Deepgram removed, ADR 0067) and, since qrtz is
/// entirely offline (ADR 0076), every cloud key: ElevenLabs talk, the summary providers (OpenRouter and the
/// others), NVIDIA ASR and the Hugging Face token. `secrets:set` refuses them.
pub const RETIRED_SECRETS: &[&str] = &[
    "asr.deepgram",
    "asr.elevenlabs",
    "asr.nvidia",
    "hf.token",
    "llm.anthropic",
    "llm.google",
    "llm.openai",
    "llm.openrouter",
];

/// Launch migration: delete a retired key the user once saved (from the file store or the Keychain), so it doesn't
/// linger unused. Only names the store lists are touched (no Keychain lookup for a key that was never saved).
pub fn drop_retired(store: &dyn SecretStore) {
    let saved = store.list();
    for name in RETIRED_SECRETS.iter().filter(|n| saved.iter().any(|s| s == *n)) {
        match store.set(name, None) {
            Ok(()) => tracing::info!(name, "retired secret deleted"),
            Err(e) => tracing::warn!(name, err = %e.message, "couldn't delete a retired secret"),
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
    /// N106: a missing file is an empty store; an unreadable or corrupt one is an error, never "empty": saving
    /// over it would drop every other key.
    fn read(&self) -> Result<BTreeMap<String, String>, String> {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(format!("couldn't read the secrets file: {}", e.kind())),
        };
        serde_json::from_str(&text).map_err(|e| format!("the secrets file is corrupt (line {})", e.line()))
    }
    /// `get` / `list` can't fail: a corrupt store reads as empty there, logged (the keys are kept on disk).
    fn read_or_empty(&self) -> BTreeMap<String, String> {
        self.read().unwrap_or_else(|e| {
            tracing::warn!(err = %e, "secret store unreadable; keys left untouched");
            BTreeMap::new()
        })
    }
}
impl SecretStore for FileSecretStore {
    fn set(&self, name: &str, value: Option<&str>) -> Result<(), AppError> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut all = self.read().map_err(|e| {
            AppError::new(
                qrtz_ipc::ErrorCode::Internal,
                format!("{e}, so the key was not saved (the file is left as it is)"),
            )
        })?;
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
        self.read_or_empty().get(name).cloned()
    }
    fn list(&self) -> Vec<String> {
        self.read_or_empty().into_keys().collect()
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
        // N52: the Keychain already holds the change; a failed index write would make secrets:list lie, so say so.
        write_private_atomic(
            &self.index,
            serde_json::to_string(&names).unwrap_or_default().as_bytes(),
        )
        .map_err(|e| {
            tracing::warn!(err = %e, index = %self.index.display(), "secret index write failed");
            AppError::new(ErrorCode::Internal, format!("secret index write failed: {e}"))
        })
    }
    fn get(&self, name: &str) -> Option<String> {
        keyring::Entry::new(&self.service, name).ok()?.get_password().ok()
    }
    fn list(&self) -> Vec<String> {
        self.names()
    }
}
