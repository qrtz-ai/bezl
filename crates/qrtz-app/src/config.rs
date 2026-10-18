//! Port of apps/desktop/src/main/config-store.ts + core/config.ts: JSON config with defaults from the zod
//! schema (embedded via the contract), field-level fallback on load, strict validation on write, atomic save.
use qrtz_ipc::{contract, AppError, ErrorCode};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub struct ConfigStore {
    file: PathBuf,
    cfg: Value,
}

/// ADR 0146 M1.2: each module's config sections live in that module's own settings file
/// (`modules/<id>/settings.json` next to config.json; `talk` is echo's, `note` and `speech` are nano's), and
/// config.json keeps the global ones. `config:get` / `config:set` still see one merged config.
fn module_store(file: &Path) -> Option<qrtz_modules::settings::ModuleSettingsStore> {
    file.parent()
        .filter(|_| !file.as_os_str().is_empty())
        .map(qrtz_modules::settings::ModuleSettingsStore::new)
}

/// Overlay each module's stored sections on the raw config.json value. Returns whether config.json still holds a
/// section that belongs in a module file (an older layout, moved on the next write).
fn overlay_modules(file: &Path, raw: &mut Map<String, Value>) -> bool {
    let Some(store) = module_store(file) else {
        return false;
    };
    let mut legacy = false;
    for m in &crate::modules::bundled().manifests {
        let stored = store.stored(&m.id).unwrap_or_default();
        for sec in &m.settings.legacy_sections {
            if raw.contains_key(sec) {
                legacy = true;
            }
            // The module's file wins: a crash between its write and config.json's leaves the newer value there.
            if let Some(v) = stored.get(sec) {
                raw.insert(sec.clone(), v.clone());
            }
        }
    }
    legacy
}

fn is_obj(v: &Value) -> bool {
    v.is_object()
}

/// zod strips unknown keys; JSON Schema (`additionalProperties: false`) would reject them. Mirror zod.
pub fn strip_unknown(v: &Value, schema: &Value) -> Value {
    match (v, schema.get("properties").and_then(Value::as_object)) {
        (Value::Object(o), Some(props)) => {
            let mut out = Map::new();
            for (k, val) in o {
                if let Some(s) = props.get(k) {
                    out.insert(k.clone(), strip_unknown(val, s));
                }
            }
            Value::Object(out)
        }
        _ => v.clone(),
    }
}

/// `deepMerge` from core/config.ts (objects merge, everything else replaces; proto keys ignored).
pub fn deep_merge(base: &Value, patch: &Value) -> Value {
    let Value::Object(p) = patch else {
        return patch.clone();
    };
    let mut out = match base {
        Value::Object(b) => b.clone(),
        _ => Map::new(),
    };
    for (k, v) in p {
        if k == "__proto__" || k == "constructor" || k == "prototype" {
            continue;
        }
        let merged = if is_obj(v) {
            deep_merge(out.get(k).unwrap_or(&Value::Object(Map::new())), v)
        } else {
            v.clone()
        };
        out.insert(k.clone(), merged);
    }
    Value::Object(out)
}

/// Legacy shapes, migrated before validation (and persisted once by [`ConfigStore::open`]):
/// - the Note sub app was called Memo: stored `memo: {...}` moves to `note` (keys under `note` win).
///
/// The removed cloud settings (ADR 0076: `talk.engine`, `talk.autoFailover`, `note.llmProvider`, `note.llmModel`,
/// `speech.cloud`, `privacy.localOnly`; ADR 0067's `talk.engine: "deepgram"`; ADR 0078's dead `app.autoUpdateCheck`
/// and `telemetry.crashReports`) need no migration: they are no longer
/// in the schema, so loading strips them like any unknown key and every other setting is kept.
pub fn migrate_legacy(raw: &Value) -> (Value, bool) {
    let Some(o) = raw.as_object() else {
        return (raw.clone(), false);
    };
    let mut rest = o.clone();
    let mut changed = false;
    if let Some(memo) = rest.remove("memo") {
        let note = deep_merge(
            if memo.is_object() { &memo } else { &Value::Null },
            rest.get("note").unwrap_or(&Value::Null),
        );
        let note = if note.is_object() {
            note
        } else {
            Value::Object(Map::new())
        };
        rest.insert("note".into(), note);
        changed = true;
    }
    (Value::Object(rest), changed)
}

/// `loadConfig`: defaults + stored values; an invalid top-level key falls back to its default alone.
pub fn load_config(input: Option<&Value>) -> Value {
    let c = contract();
    let schema = &c.raw["config"]["schema"];
    let raw = input
        .map(|v| migrate_legacy(v).0)
        .unwrap_or(Value::Object(Map::new()));
    let stored = strip_unknown(&raw, schema);
    let merged = deep_merge(&c.config_defaults, &stored);
    if c.config_schema.is_valid(&merged) {
        return merged;
    }
    let mut obj = stored.as_object().cloned().unwrap_or_default();
    for e in c.config_schema.iter_errors(&merged) {
        let path = e.instance_path().to_string();
        if let Some(top) = path
            .trim_start_matches('/')
            .split('/')
            .next()
            .filter(|s| !s.is_empty())
        {
            obj.remove(top);
        }
    }
    let retry = deep_merge(&c.config_defaults, &Value::Object(obj));
    if c.config_schema.is_valid(&retry) {
        retry
    } else {
        c.config_defaults.clone()
    }
}

impl ConfigStore {
    pub fn open(file: impl AsRef<Path>) -> Self {
        let file = file.as_ref().to_path_buf();
        let raw: Option<Value> = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        let memo = raw.as_ref().is_some_and(|r| migrate_legacy(r).1);
        let mut merged = raw
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let legacy = overlay_modules(&file, &mut merged);
        let any = raw.is_some() || !merged.is_empty();
        let cfg = load_config(any.then_some(&Value::Object(merged)));
        let mut s = Self { file, cfg };
        if memo || legacy {
            let _ = s.persist();
        }
        s
    }

    pub fn in_memory() -> Self {
        Self {
            file: PathBuf::new(),
            cfg: load_config(None),
        }
    }

    pub fn get(&self) -> &Value {
        &self.cfg
    }

    /// `applyConfigPatch`: merge and validate strictly (writes must be valid). Persists atomically.
    pub fn set(&mut self, patch: &Value) -> Result<Value, AppError> {
        let c = contract();
        let schema = &c.raw["config"]["schema"];
        let next = strip_unknown(&deep_merge(&self.cfg, patch), schema);
        if !c.config_schema.is_valid(&next) {
            return Err(AppError::new(ErrorCode::Validation, "Invalid config")
                .details(Value::Array(qrtz_ipc::contract::issues(&c.config_schema, &next))));
        }
        self.cfg = next;
        self.persist()
            .map_err(|e| AppError::new(ErrorCode::Internal, format!("config write failed: {e}")))?;
        Ok(self.cfg.clone())
    }

    fn persist(&mut self) -> std::io::Result<()> {
        if self.file.as_os_str().is_empty() {
            return Ok(());
        }
        // ADR 0146 M1.2: module sections first (their file wins on load), then the global rest.
        let split = qrtz_modules::settings::split_legacy(&self.cfg, &crate::modules::bundled().manifests);
        if let Some(store) = module_store(&self.file) {
            for (id, sections) in &split.modules {
                let mut v = store.stored(id).unwrap_or_default();
                for (k, sv) in sections.as_object().into_iter().flatten() {
                    v.insert(k.clone(), sv.clone());
                }
                store
                    .replace(id, &Value::Object(v))
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
            }
        }
        let mut global = split.global.as_object().cloned().unwrap_or_default();
        for k in &split.unowned {
            global.insert(k.clone(), self.cfg[k].clone());
        }
        // N25 dir 0700 + T4 stale-tmp removal + 0600 atomic rename (shared with the secret store).
        crate::secrets::write_private_atomic(
            &self.file,
            serde_json::to_string_pretty(&Value::Object(global))?.as_bytes(),
        )
    }

    /// Typed accessors the shell uses to apply toggles.
    pub fn bool_at(&self, path: &[&str]) -> bool {
        let mut v = &self.cfg;
        for p in path {
            v = &v[*p];
        }
        v.as_bool().unwrap_or(false)
    }
    pub fn str_at(&self, path: &[&str]) -> String {
        let mut v = &self.cfg;
        for p in path {
            v = &v[*p];
        }
        v.as_str().unwrap_or_default().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(p: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    /// ADR 0146 M1.2: an old single-file config moves its module sections into the modules' files once; the
    /// merged config is unchanged.
    #[test]
    fn module_sections_move_into_their_modules_files() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("config.json");
        let old = json!({"theme": "light", "talk": {"pushKey": "right-option"}, "note": {"autoRecord": true},
                         "speech": {"speakerMode": "accurate"}, "privacy": {"excludedApps": ["Slack"]}});
        std::fs::write(&f, old.to_string()).unwrap();
        let before = load_config(Some(&old));
        let s = ConfigStore::open(&f);
        assert_eq!(s.get(), &before);
        let global = read(&f);
        for k in ["talk", "note", "speech"] {
            assert!(global.get(k).is_none(), "{k} still in config.json: {global}");
        }
        assert_eq!(global["theme"], "light");
        assert_eq!(global["privacy"]["excludedApps"], json!(["Slack"]));
        let echo = read(&dir.path().join("modules/echo/settings.json"));
        assert_eq!(echo["talk"]["pushKey"], "right-option");
        let nano = read(&dir.path().join("modules/nano/settings.json"));
        assert_eq!(nano["note"]["autoRecord"], true);
        assert_eq!(nano["speech"]["speakerMode"], "accurate");
        assert!(!dir.path().join("modules/ctrl/settings.json").exists());
        assert_eq!(ConfigStore::open(&f).get(), &before, "reload");
    }

    #[test]
    fn a_write_lands_in_the_owning_file_and_the_modules_file_wins_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("config.json");
        let mut s = ConfigStore::open(&f);
        s.set(&json!({"talk": {"showPill": false}, "theme": "light"}))
            .unwrap();
        assert_eq!(
            read(&dir.path().join("modules/echo/settings.json"))["talk"]["showPill"],
            false
        );
        assert!(read(&f).get("talk").is_none());
        // A stale talk section left in config.json (crash mid-write, or an older build) loses to echo's file.
        let mut g = read(&f);
        g["talk"] = json!({"showPill": true});
        std::fs::write(&f, g.to_string()).unwrap();
        let s = ConfigStore::open(&f);
        assert!(!s.bool_at(&["talk", "showPill"]));
        assert!(read(&f).get("talk").is_none(), "moved again");
        // Other keys a module keeps in its file survive the shell's writes.
        let p = dir.path().join("modules/echo/settings.json");
        let mut e = read(&p);
        e["native"] = json!({"x": 1});
        std::fs::write(&p, e.to_string()).unwrap();
        let mut s = ConfigStore::open(&f);
        s.set(&json!({"talk": {"warmMic": true}})).unwrap();
        assert_eq!(read(&p)["native"]["x"], 1);
        assert_eq!(read(&p)["talk"]["warmMic"], true);
    }

    /// ADR 0076 (and ADR 0067): a config from before qrtz went fully offline loads. The cloud settings are dropped
    /// (unknown keys), every other setting is kept, and nothing needs rewriting.
    #[test]
    fn an_old_cloud_config_loads_without_its_cloud_settings() {
        let old = json!({"talk": {"engine": "deepgram", "autoFailover": false, "showPill": false,
                                  "pushKey": "right-option"},
                         "note": {"llmProvider": "openrouter", "llmModel": "anthropic/claude-haiku-5.5",
                                  "autoSummarize": false},
                         "speech": {"cloud": "elevenlabs", "speakerMode": "accurate"},
                         "privacy": {"localOnly": true, "excludedApps": ["Slack"]},
                         "app": {"autoUpdateCheck": false, "launchAtLogin": true},
                         "telemetry": {"crashReports": true}});
        assert!(!migrate_legacy(&old).1);
        let cfg = load_config(Some(&old));
        for (sec, k) in [
            ("talk", "engine"),
            ("talk", "autoFailover"),
            ("note", "llmProvider"),
            ("note", "llmModel"),
            ("speech", "cloud"),
            ("privacy", "localOnly"),
            ("app", "autoUpdateCheck"),
        ] {
            assert!(cfg[sec].get(k).is_none(), "{sec}.{k} in {cfg}");
        }
        // ADR 0078: the dead update-check and crash-report settings are gone too.
        assert!(cfg.get("telemetry").is_none(), "{cfg}");
        assert_eq!(cfg["app"]["launchAtLogin"], true, "{cfg}");
        assert_eq!(cfg["talk"]["showPill"], false, "{cfg}");
        assert_eq!(cfg["talk"]["pushKey"], "right-option", "{cfg}");
        assert_eq!(cfg["note"]["autoSummarize"], false, "{cfg}");
        assert_eq!(cfg["speech"]["speakerMode"], "accurate", "{cfg}");
        assert_eq!(cfg["privacy"]["excludedApps"], json!(["Slack"]), "{cfg}");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("config.json");
        std::fs::write(&f, old.to_string()).unwrap();
        assert_eq!(ConfigStore::open(&f).get(), &cfg);
    }
}
