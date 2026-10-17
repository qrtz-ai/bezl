//! Secrets (LLM / ASR API keys). macOS: Keychain via `keyring`, with a names-only index file (values never
//! touch disk outside the Keychain). Elsewhere (Linux dev/test builds): process memory only.
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
        let _ = std::fs::write(&self.index, serde_json::to_string(&names).unwrap_or_default());
        Ok(())
    }
    fn get(&self, name: &str) -> Option<String> {
        keyring::Entry::new(&self.service, name).ok()?.get_password().ok()
    }
    fn list(&self) -> Vec<String> {
        self.names()
    }
}
