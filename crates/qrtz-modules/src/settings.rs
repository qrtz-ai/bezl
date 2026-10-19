//! Settings (ADR 0146 §6, Jared 2026-10-18: "each module has its own settings; the global settings only control
//! all of them").
//!
//! - **Module settings** belong to one module and live in its own file, `modules/<id>/settings.json` under the
//!   app data dir. The module's view edits them; nothing else does.
//! - **Global settings** are the few that apply across modules: the theme, the log level, the app's menu bar and
//!   login item, onboarding (ADR 0184), the speech downloads ([`GLOBAL_KEYS`]), and the `modules` switches (one per module,
//!   plus all of them at once). They stay in config.json.
//!
//! M0 adds the `modules` switches to config.json and this split. Today's per-product config sections
//! (`talk`, `note`, `speech`) move into their module's file in M1 with [`split_legacy`]; until then they keep
//! working where they are.
use crate::manifest::ModuleManifest;
use crate::ModuleError;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// config.json sections that stay global. Every other section belongs to exactly one module
/// (its manifest's `settings.legacySections`).
pub const GLOBAL_SECTIONS: &[&str] = &[
    "schemaVersion",
    "theme",
    "logLevel",
    "app",
    "onboarding",
    "modules",
];

/// Keys inside a module's section that stay global in config.json (ADR 0146 M2.3). The speech downloads
/// (`speech.autoUpdate`, `speech.warmAfterDownload`) serve echo and nano alike and are shown in the global
/// settings, while the rest of `speech` (`speakerMode`) is nano's.
pub const GLOBAL_KEYS: &[(&str, &str)] = &[("speech", "autoUpdate"), ("speech", "warmAfterDownload")];

/// The global keys of config section `section` (empty for most).
pub fn global_keys_of(section: &str) -> impl Iterator<Item = &'static str> + '_ {
    GLOBAL_KEYS
        .iter()
        .filter(move |(s, _)| *s == section)
        .map(|(_, k)| *k)
}
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

/// The global switches that control every module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalSettings {
    /// `modules.enabled`: off stops every module at once.
    pub all_enabled: bool,
    /// `modules.<id>.enabled`.
    pub enabled: BTreeMap<String, bool>,
    /// `modules.<id>.installed` (ADR 0169): qrtz ships empty, and the host writes this from `qrtz.lock` (an
    /// extension the lock resolves is installed). Missing means installed, so a config the host never synced (tests,
    /// the simulated helper) keeps every module, as before.
    pub installed: BTreeMap<String, bool>,
}

impl GlobalSettings {
    /// From config.json (missing keys default to on, so M0 changes nothing).
    pub fn from_config(cfg: &Value) -> Self {
        let m = &cfg["modules"];
        Self {
            all_enabled: m["enabled"].as_bool().unwrap_or(true),
            enabled: crate::MODULE_IDS
                .iter()
                .map(|id| ((*id).to_owned(), m[*id]["enabled"].as_bool().unwrap_or(true)))
                .collect(),
            installed: crate::MODULE_IDS
                .iter()
                .map(|id| ((*id).to_owned(), m[*id]["installed"].as_bool().unwrap_or(true)))
                .collect(),
        }
    }
    /// Installed (ADR 0169), switched on, and not under the "all off" switch.
    pub fn is_on(&self, id: &str) -> bool {
        self.all_enabled && self.is_installed(id) && self.enabled.get(id).copied().unwrap_or(false)
    }
    pub fn is_installed(&self, id: &str) -> bool {
        self.installed.get(id).copied().unwrap_or(false)
    }
}

/// Split a config.json value into the global part and each module's part (by the manifests' legacy sections).
/// A section nobody claims is returned in `unowned` so a test can fail on it.
pub struct Split {
    pub global: Value,
    pub modules: BTreeMap<String, Value>,
    pub unowned: Vec<String>,
}

pub fn split_legacy(cfg: &Value, manifests: &[ModuleManifest]) -> Split {
    let mut global = Map::new();
    let mut modules: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut unowned = vec![];
    for (k, v) in cfg.as_object().into_iter().flatten() {
        if GLOBAL_SECTIONS.contains(&k.as_str()) {
            global.insert(k.clone(), v.clone());
        } else if let Some(m) = manifests.iter().find(|m| m.settings.legacy_sections.contains(k)) {
            // M2.3: a section's global keys go to config.json, the rest to the module.
            let mut v = v.clone();
            if let Some(o) = v.as_object_mut() {
                let g: Map<String, Value> = global_keys_of(k)
                    .filter_map(|gk| o.remove(gk).map(|x| (gk.to_owned(), x)))
                    .collect();
                if !g.is_empty() {
                    global.insert(k.clone(), Value::Object(g));
                }
            }
            modules.entry(m.id.clone()).or_default().insert(k.clone(), v);
        } else {
            unowned.push(k.clone());
        }
    }
    Split {
        global: Value::Object(global),
        modules: modules.into_iter().map(|(k, v)| (k, Value::Object(v))).collect(),
        unowned,
    }
}

fn merge(base: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (k, v) in patch {
        if k == "__proto__" || k == "constructor" || k == "prototype" {
            continue;
        }
        match (base.get_mut(k), v) {
            (Some(Value::Object(b)), Value::Object(p)) => merge(b, p),
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Each module's own settings file. Validation of the values is the module's (its schema ships with its view).
pub struct ModuleSettingsStore {
    root: PathBuf,
}

impl ModuleSettingsStore {
    /// `root` is the app data dir (`~/Library/Application Support/ai.qrtz.desktop`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self, id: &str) -> Result<PathBuf, ModuleError> {
        if !crate::MODULE_IDS.contains(&id) {
            return Err(ModuleError::invalid(
                "module",
                format!("{id:?} is not a qrtz module"),
            ));
        }
        Ok(self.root.join("modules").join(id).join("settings.json"))
    }

    /// The stored object, or `{}`. A corrupt file reads as `{}` (the module's defaults) and is left in place.
    pub fn get(&self, id: &str) -> Result<Value, ModuleError> {
        let p = self.path(id)?;
        Ok(std::fs::read(&p)
            .ok()
            .filter(|b| b.len() <= MAX_SETTINGS_BYTES)
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Map::new())))
    }

    /// Merge `patch` into module `id`'s settings. A global key can't be set from a module.
    pub fn set(&self, id: &str, patch: &Value) -> Result<Value, ModuleError> {
        let Some(p) = patch.as_object() else {
            return Err(ModuleError::invalid("settings", "an object"));
        };
        if let Some(k) = p.keys().find(|k| GLOBAL_SECTIONS.contains(&k.as_str())) {
            return Err(ModuleError::invalid(
                format!("settings.{k}"),
                "a global setting; change it in the global settings",
            ));
        }
        for (sec, v) in p {
            if let Some(gk) = global_keys_of(sec).find(|gk| v.get(*gk).is_some()) {
                return Err(ModuleError::invalid(
                    format!("settings.{sec}.{gk}"),
                    "a global setting; change it in the global settings",
                ));
            }
        }
        let mut cur = match self.get(id)? {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        merge(&mut cur, p);
        let out = Value::Object(cur);
        let bytes = serde_json::to_vec_pretty(&out).expect("json");
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(ModuleError::invalid("settings", "too large"));
        }
        write_private(&self.path(id)?, &bytes)?;
        Ok(out)
    }

    /// Replace module `id`'s whole settings object (the shell's config store writes a module's sections this way).
    /// An object, no global sections, size-capped. A [`GLOBAL_KEYS`] key may stay in it for the length of one
    /// config write: the shell keeps the old copy until config.json has the key, then removes it (M2.3).
    pub fn replace(&self, id: &str, value: &Value) -> Result<(), ModuleError> {
        let Some(o) = value.as_object() else {
            return Err(ModuleError::invalid("settings", "an object"));
        };
        if let Some(k) = o.keys().find(|k| GLOBAL_SECTIONS.contains(&k.as_str())) {
            return Err(ModuleError::invalid(
                format!("settings.{k}"),
                "a global setting; change it in the global settings",
            ));
        }
        let bytes = serde_json::to_vec_pretty(value).expect("json");
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(ModuleError::invalid("settings", "too large"));
        }
        write_private(&self.path(id)?, &bytes)
    }

    /// The stored object if the file exists and parses (`None` otherwise, unlike [`ModuleSettingsStore::get`]).
    pub fn stored(&self, id: &str) -> Option<Map<String, Value>> {
        let b = std::fs::read(self.path(id).ok()?).ok()?;
        if b.len() > MAX_SETTINGS_BYTES {
            return None;
        }
        match serde_json::from_slice(&b).ok()? {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// What a module sees: its own settings plus a read-only copy of the global ones (theme, log level, its
    /// switch). The module never writes through this.
    pub fn effective(&self, id: &str, global_cfg: &Value) -> Result<Value, ModuleError> {
        let g = GlobalSettings::from_config(global_cfg);
        let mut shared = Map::new();
        for k in ["theme", "logLevel"] {
            if let Some(v) = global_cfg.get(k) {
                shared.insert(k.into(), v.clone());
            }
        }
        shared.insert("enabled".into(), Value::Bool(g.is_on(id)));
        Ok(serde_json::json!({ "global": shared, "module": self.get(id)? }))
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ModuleError> {
    use std::io::Write;
    let dir = path.parent().expect("settings path has a dir");
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let tmp = dir.join(".settings.json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_sections_global_keys_split_into_the_global_part() {
        let m = ModuleManifest::parse(crate::bundled::NANO.as_bytes()).unwrap();
        let cfg = json!({"theme": "dark", "speech": {"speakerMode": "accurate", "autoUpdate": false},
                         "privacy": {"excludedApps": ["Slack"]}});
        let s = split_legacy(&cfg, &[m]);
        assert!(s.unowned.is_empty());
        assert_eq!(
            s.global,
            json!({"theme": "dark", "speech": {"autoUpdate": false}})
        );
        assert_eq!(
            s.modules["nano"],
            json!({"speech": {"speakerMode": "accurate"}, "privacy": {"excludedApps": ["Slack"]}})
        );
    }

    #[test]
    fn a_module_cant_set_a_global_key() {
        let dir = tempfile::tempdir().unwrap();
        let st = ModuleSettingsStore::new(dir.path());
        let e = st
            .set("nano", &json!({"speech": {"autoUpdate": false}}))
            .unwrap_err();
        assert!(e.to_string().contains("speech.autoUpdate"), "{e}");
        st.set(
            "nano",
            &json!({"speech": {"speakerMode": "fast"}, "privacy": {"pauseHotkey": ""}}),
        )
        .unwrap();
    }
}
