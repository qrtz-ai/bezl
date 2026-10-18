//! `qrtz.json`, the manifest inside every package (ADR 0157 §3), and package names.
//!
//! ```json
//! {"name": "echo", "version": "0.3.0", "kind": "extension",
//!  "dependencies": {"inference.voice": "^1.0.0", "model.parakeet-tdt-v2-en": "^1.0.0"},
//!  "extension": {"entry": "bin/qrtz-echo", "protocol": 1, "minApp": "0.3.0", ...}}
//! ```
//!
//! - **Names** are lowercase dotted words. The kind is in the name: `model.<name>` is a model, `inference.<name>` a
//!   service, and an extension is one of [`crate::EXTENSION_NAMES`] (an extension runs inside qrtz's TCC identity,
//!   so adding one takes a shell release).
//! - **Dependencies** are semver ranges (`^1.0.0`, `~1.2`, `>=1.0.0, <2.0.0`), resolved only online against the
//!   signed index; the exact result goes in `qrtz.lock`. Only extensions have dependencies, and only on services and
//!   models: services and models depend on nothing, and nothing depends on an extension.
//! - **No install scripts.** There is no field that runs anything at install time; `scripts`, `preinstall`,
//!   `install`, `postinstall` and `prepare` are refused by name ([`ModuleError::InstallScript`]). Installing is
//!   copying verified bytes and nothing else.
//! - Team id and designated requirement are not in `qrtz.json`: they come from the signature and the signed index
//!   entry, and are recorded in `qrtz.lock`.
use crate::index::PackageKind;
use crate::ModuleError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_QRTZ_JSON_BYTES: usize = 64 * 1024;
/// Fields that would run code at install time in other package managers.
pub const SCRIPT_FIELDS: &[&str] = &[
    "scripts",
    "preinstall",
    "install",
    "postinstall",
    "prepare",
    "hooks",
];

/// `echo`, `inference.voice`, `model.parakeet-tdt-v2-en`: dot-separated words of `[a-z0-9-]`, each starting with a
/// letter or digit, the first with a letter; at most 64 bytes.
pub fn check_name(name: &str) -> Result<(), ModuleError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_lowercase()
        && name.split('.').all(|w| {
            !w.is_empty()
                && w.as_bytes()[0].is_ascii_alphanumeric()
                && w.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
    if ok {
        Ok(())
    } else {
        Err(ModuleError::invalid(
            "name",
            format!("{name:?}: lowercase dotted words"),
        ))
    }
}

/// The kind a name says.
pub fn kind_of(name: &str) -> Result<PackageKind, ModuleError> {
    check_name(name)?;
    if name.strip_prefix("model.").is_some_and(|r| !r.is_empty()) {
        Ok(PackageKind::Model)
    } else if name.strip_prefix("inference.").is_some_and(|r| !r.is_empty()) {
        Ok(PackageKind::Service)
    } else if crate::EXTENSION_NAMES.contains(&name) {
        Ok(PackageKind::Extension)
    } else if name == "qrtz" {
        Ok(PackageKind::Shell)
    } else {
        Err(ModuleError::invalid(
            "name",
            format!("{name:?}: model.<name>, inference.<name>, or one of the extensions qrtz knows"),
        ))
    }
}

/// The code identity a package of this name must be signed as (`None` for models).
pub fn identifier_for(name: &str, kind: PackageKind) -> Option<String> {
    match kind {
        // ADR 0146's bundle ids (`ai.qrtz.echo`), kept as package signing identities (ADR 0154 §1).
        PackageKind::Extension | PackageKind::Service => Some(format!("ai.qrtz.{name}")),
        PackageKind::Shell => Some("ai.qrtz.desktop".into()),
        PackageKind::Model => None,
    }
}

/// A dependency range: what `semver::VersionReq` parses, minus build metadata. Tags, URLs, paths and git refs are
/// not ranges and are refused.
pub fn parse_range(field: &str, r: &str) -> Result<semver::VersionReq, ModuleError> {
    if r.contains('+') || r.contains(':') || r.contains('/') || r.trim() != r || r.is_empty() {
        return Err(ModuleError::invalid(field, format!("{r:?}: a semver range")));
    }
    semver::VersionReq::parse(r).map_err(|e| ModuleError::invalid(field, format!("{r:?}: {e}")))
}

/// The dependency rules (ADR 0157 §3): only extensions depend, only on services and models, each a valid range.
pub fn check_dependencies(
    name: &str,
    kind: PackageKind,
    deps: &BTreeMap<String, String>,
) -> Result<(), ModuleError> {
    if kind != PackageKind::Extension && !deps.is_empty() {
        return Err(ModuleError::invalid(
            format!("{name}.dependencies"),
            "only extensions have dependencies",
        ));
    }
    for (d, r) in deps {
        match kind_of(d)? {
            PackageKind::Service | PackageKind::Model => {}
            k => {
                return Err(ModuleError::invalid(
                    format!("{name}.dependencies.{d}"),
                    format!("a {k:?} can't be a dependency"),
                ))
            }
        }
        parse_range(&format!("{name}.dependencies.{d}"), r)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub kind: PackageKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<Extension>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<Service>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
}

/// An extension (ADR 0157 §2): a helper executable qrtz spawns directly. It has no bundle id of its own as far as
/// TCC is concerned: macOS attributes its microphone, accessibility and other prompts to qrtz.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Extension {
    /// The helper executable, relative to the package dir (a plain Mach-O, not an `.app`).
    pub entry: String,
    /// The helper protocol version (contracts/helper-protocol.json).
    pub protocol: u32,
    pub min_app: String,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub views: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Which of qrtz's TCC grants the extension uses (shown in the extensions screen; there is one qrtz entry in
    /// System Settings, not one per extension).
    #[serde(default)]
    pub permissions: Vec<crate::manifest::Permission>,
    #[serde(default)]
    pub settings: crate::manifest::SettingsDecl,
    /// Models the extension's own process reads (ctrl's `model.laya` until an inference service runs it). Each
    /// must also be one of its dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
}

/// A sandboxed XPC service (ADR 0144 §2, 0157 §7): no network, no TCC, models only by handle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Service {
    /// The `.xpc` bundle, relative to the package dir.
    pub bundle: String,
    /// Models it may be handed at connect (a file descriptor or a sandbox extension token, never a path it opens
    /// itself).
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model {
    /// `asr`, `diarization`, `echo-cancellation`, `decision`.
    pub task: String,
    /// Where the weights come from (`FluidInference/parakeet-tdt-0.6b-v2-coreml`) and the exact upstream revision.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub upstream: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
}

fn is_word(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

impl PackageManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, ModuleError> {
        if bytes.len() > MAX_QRTZ_JSON_BYTES {
            return Err(ModuleError::invalid("qrtz.json", "too large"));
        }
        let raw: Value = serde_json::from_slice(bytes).map_err(|e| ModuleError::Json(e.to_string()))?;
        let mut found = vec![];
        collect_script_fields(&raw, &mut found);
        if let Some(f) = found.into_iter().next() {
            return Err(ModuleError::InstallScript(f));
        }
        let m: Self = serde_json::from_value(raw).map_err(|e| ModuleError::Json(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn semver(&self) -> semver::Version {
        semver::Version::parse(&self.version).expect("validated")
    }

    pub fn validate(&self) -> Result<(), ModuleError> {
        let kind = kind_of(&self.name)?;
        if kind != self.kind {
            return Err(ModuleError::invalid(
                "kind",
                format!("{} is a {kind:?} by its name", self.name),
            ));
        }
        crate::manifest::exact_version("version", &self.version)?;
        check_dependencies(&self.name, kind, &self.dependencies)?;
        let sections = (
            self.extension.is_some(),
            self.service.is_some(),
            self.model.is_some(),
        );
        let want = match kind {
            PackageKind::Extension => (true, false, false),
            PackageKind::Service => (false, true, false),
            PackageKind::Model => (false, false, true),
            PackageKind::Shell => (false, false, false),
        };
        if sections != want {
            return Err(ModuleError::invalid(
                "qrtz.json",
                format!("a {kind:?} has exactly its own section"),
            ));
        }
        if let Some(e) = &self.extension {
            if !safe_rel(&e.entry) {
                return Err(ModuleError::invalid("extension.entry", "a relative path"));
            }
            crate::manifest::exact_version("extension.minApp", &e.min_app)?;
            for ch in &e.channels {
                if !(ch.ends_with(".*") && is_word(&ch[..ch.len() - 2])) {
                    return Err(ModuleError::invalid("extension.channels", format!("{ch:?}")));
                }
            }
            for v in &e.views {
                if !is_word(v) {
                    return Err(ModuleError::invalid("extension.views", format!("{v:?}")));
                }
            }
            if !e.settings.namespace.is_empty() && e.settings.namespace != self.name {
                return Err(ModuleError::invalid(
                    "extension.settings.namespace",
                    "the extension name",
                ));
            }
            for r in &e.reads {
                if kind_of(r).ok() != Some(PackageKind::Model) || !self.dependencies.contains_key(r) {
                    return Err(ModuleError::invalid(
                        "extension.reads",
                        format!("{r}: one of its model dependencies"),
                    ));
                }
            }
        }
        if let Some(s) = &self.service {
            if !safe_rel(&s.bundle) || !s.bundle.ends_with(".xpc") {
                return Err(ModuleError::invalid(
                    "service.bundle",
                    "a relative path to an .xpc",
                ));
            }
            for m in &s.models {
                if kind_of(m).ok() != Some(PackageKind::Model) {
                    return Err(ModuleError::invalid("service.models", format!("{m}: a model")));
                }
            }
        }
        if let Some(m) = &self.model {
            if !is_word(&m.task) {
                return Err(ModuleError::invalid("model.task", "a lowercase word"));
            }
        }
        Ok(())
    }

    /// The index entry facts this manifest fixes (checked against the entry when a package is staged).
    pub fn task(&self) -> Option<&str> {
        self.model.as_ref().map(|m| m.task.as_str())
    }
    pub fn service_models(&self) -> &[String] {
        self.service.as_ref().map(|s| s.models.as_slice()).unwrap_or(&[])
    }
}

fn collect_script_fields(v: &Value, out: &mut Vec<String>) {
    if let Value::Object(o) = v {
        for (k, x) in o {
            if SCRIPT_FIELDS.contains(&k.as_str()) {
                out.push(k.clone());
            }
            collect_script_fields(x, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_say_their_kind() {
        assert_eq!(kind_of("echo").unwrap(), PackageKind::Extension);
        assert_eq!(kind_of("inference.voice").unwrap(), PackageKind::Service);
        assert_eq!(kind_of("model.parakeet-tdt-v2-en").unwrap(), PackageKind::Model);
        for bad in [
            "Echo",
            "model.",
            "inference",
            "evil",
            "model..x",
            "model.-x",
            "",
            ".echo",
            "echo/x",
        ] {
            assert!(kind_of(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ranges_only() {
        for ok in ["^1.0.0", "~1.2", ">=1.0.0, <2.0.0", "1.0.0", "=1.2.3"] {
            parse_range("d", ok).unwrap();
        }
        for bad in [
            "latest",
            "github:x/y",
            "file:../x",
            "^1.0.0+abc",
            "",
            " ^1",
            "1.0.0 || 2.0.0",
        ] {
            assert!(parse_range("d", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn install_scripts_are_refused_by_name() {
        let m = br#"{"name":"model.x","version":"1.0.0","kind":"model","model":{"task":"asr"},"scripts":{"postinstall":"curl x | sh"}}"#;
        assert!(matches!(PackageManifest::parse(m), Err(ModuleError::InstallScript(f)) if f == "scripts"));
        let m = br#"{"name":"echo","version":"1.0.0","kind":"extension","extension":{"entry":"bin/e","protocol":1,"minApp":"0.3.0","postinstall":"x"}}"#;
        assert!(matches!(
            PackageManifest::parse(m),
            Err(ModuleError::InstallScript(_))
        ));
    }

    #[test]
    fn only_extensions_depend_and_only_on_services_and_models() {
        let ok = br#"{"name":"echo","version":"1.0.0","kind":"extension","dependencies":{"inference.voice":"^1.0.0","model.parakeet-tdt-v2-en":"^1.0.0"},"extension":{"entry":"bin/qrtz-echo","protocol":1,"minApp":"0.3.0"}}"#;
        PackageManifest::parse(ok).unwrap();
        let on_ext = br#"{"name":"echo","version":"1.0.0","kind":"extension","dependencies":{"nano":"^1.0.0"},"extension":{"entry":"bin/e","protocol":1,"minApp":"0.3.0"}}"#;
        assert!(PackageManifest::parse(on_ext).is_err());
        let svc = br#"{"name":"inference.voice","version":"1.0.0","kind":"service","dependencies":{"model.x":"^1"},"service":{"bundle":"v.xpc"}}"#;
        assert!(PackageManifest::parse(svc).is_err());
    }
}
