//! API keys for the providers. The values come from the app's secrets store (owner-only 0600 and written
//! atomically, N30/N33 0ac7fa0; or the macOS Keychain), which qrtz-app owns and exposes through [`SecretSource`].
//! A key is held as a [`Secret`]: its `Debug` / `Display` never print it, it goes into one request header only, and
//! provider errors that echo it are redacted, so a key never reaches a log line or an error message.
use std::sync::Arc;

/// A secret value. Printing it (`{:?}` / `{}`) shows `Secret(***)`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(v: impl Into<String>) -> Secret {
        Secret(v.into())
    }
    /// The raw value, for the request header only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}
impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

/// Read access to the secrets store (`SecretStore.get`). Names are the TS `SECRET_NAMES` (`llm.openrouter`, ...).
/// Any `Fn(&str) -> Option<String>` is one, e.g. `move |n| app_store.get(n)`.
pub trait SecretSource: Send + Sync {
    fn get(&self, name: &str) -> Option<Secret>;
}

impl<F: Fn(&str) -> Option<String> + Send + Sync> SecretSource for F {
    fn get(&self, name: &str) -> Option<Secret> {
        self(name).filter(|v| !v.is_empty()).map(Secret)
    }
}

/// `envNameFor`: `llm.openai` → `QRTZ_SECRET_LLM_OPENAI`.
pub fn env_name_for(name: &str) -> String {
    let up: String = name
        .to_uppercase()
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() || c.is_ascii_digit() {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("QRTZ_SECRET_{up}")
}

/// `envNamesFor`: also the short form without `llm.` (`QRTZ_SECRET_OPENROUTER`).
pub fn env_names_for(name: &str) -> Vec<String> {
    let mut v = vec![env_name_for(name)];
    if let Some(rest) = name.strip_prefix("llm.") {
        v.push(env_name_for(rest));
    }
    v
}

/// An environment lookup (`std::env::var` in the app).
pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The env override of the Electron store (`QRTZ_SECRET_*` win, for headless and dev runs), over any store.
pub struct EnvOverride<S> {
    pub inner: S,
    pub env: EnvFn,
}

impl<S: SecretSource> EnvOverride<S> {
    /// Over the process environment.
    pub fn process(inner: S) -> Self {
        EnvOverride {
            inner,
            env: Arc::new(|k| std::env::var(k).ok()),
        }
    }
}

impl<S: SecretSource> SecretSource for EnvOverride<S> {
    fn get(&self, name: &str) -> Option<Secret> {
        env_names_for(name)
            .iter()
            .find_map(|n| (self.env)(n).filter(|v| !v.is_empty()))
            .map(Secret)
            .or_else(|| self.inner.get(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn never_printed_and_env_names() {
        let s = Secret::new("sk-or-v1-abcdef");
        assert_eq!(format!("{s:?} {s}"), "Secret(***) ***");
        assert_eq!(
            env_names_for("llm.openrouter"),
            ["QRTZ_SECRET_LLM_OPENROUTER", "QRTZ_SECRET_OPENROUTER"]
        );
        assert_eq!(env_names_for("asr.nvidia"), ["QRTZ_SECRET_ASR_NVIDIA"]);
        let store = |n: &str| (n == "llm.openrouter").then(|| "stored".to_string());
        let e = EnvOverride {
            inner: store,
            env: Arc::new(|k| (k == "QRTZ_SECRET_OPENROUTER").then(|| "env".to_string())),
        };
        assert_eq!(e.get("llm.openrouter").unwrap().expose(), "env");
        let e = EnvOverride {
            inner: store,
            env: Arc::new(|_| None),
        };
        assert_eq!(e.get("llm.openrouter").unwrap().expose(), "stored");
        assert!(e.get("llm.openai").is_none());
    }
}
