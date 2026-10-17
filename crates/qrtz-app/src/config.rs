//! Port of apps/desktop/src/main/config-store.ts + core/config.ts: JSON config with defaults from the zod
//! schema (embedded via the contract), field-level fallback on load, strict validation on write, atomic save.
use qrtz_ipc::{contract, AppError, ErrorCode};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub struct ConfigStore {
    file: PathBuf,
    cfg: Value,
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

/// The Note sub app was called Memo: stored `memo: {...}` moves to `note` (keys under `note` win).
pub fn migrate_legacy(raw: &Value) -> (Value, bool) {
    let Some(o) = raw.as_object() else {
        return (raw.clone(), false);
    };
    let Some(memo) = o.get("memo") else {
        return (raw.clone(), false);
    };
    let mut rest = o.clone();
    rest.remove("memo");
    let note = deep_merge(
        if memo.is_object() { memo } else { &Value::Null },
        rest.get("note").unwrap_or(&Value::Null),
    );
    let note = if note.is_object() {
        note
    } else {
        Value::Object(Map::new())
    };
    rest.insert("note".into(), note);
    (Value::Object(rest), true)
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
        let cfg = load_config(raw.as_ref());
        let mut s = Self { file, cfg };
        if raw.as_ref().is_some_and(|r| migrate_legacy(r).1) {
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
        // N25 dir 0700 + T4 stale-tmp removal + 0600 atomic rename (shared with the secret store).
        crate::secrets::write_private_atomic(&self.file, serde_json::to_string_pretty(&self.cfg)?.as_bytes())
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
