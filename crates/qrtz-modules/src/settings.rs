//! Settings (ADR 0146 §6, Jared 2026-10-18: "each module has its own settings; the global settings only control
//! all of them").
//!
//! - **Module settings** belong to one module and live in its own file, `modules/<id>/settings.json` under the
//!   app data dir. The module's view edits them; nothing else does.
//! - **Global settings** are the few that apply across modules: the theme, the log level, the app's menu bar and
//!   login item, onboarding, privacy, and the `modules` switches (one per module, plus all of them at once). They
//!   stay in config.json.
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
    "privacy",
    "modules",
];
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

/// The global switches that control every module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalSettings {
    /// `modules.enabled`: off stops every module at once.
    pub all_enabled: bool,
    /// `modules.<id>.enabled`.
    pub enabled: BTreeMap<String, bool>,
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
        }
    }
    pub fn is_on(&self, id: &str) -> bool {
        self.all_enabled && self.enabled.get(id).copied().unwrap_or(false)
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
            modules
                .entry(m.id.clone())
                .or_default()
                .insert(k.clone(), v.clone());
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
