//! The module manifest (ADR 0146). One per module, `modules/<id>/module.json` in the repo and
//! `modules/<id>/versions/<v>/module.json` once installed. In M7 it is the per-version entry of the signed index;
//! the shell only trusts it after the index signature (ADR 0046 verifier) and the sha256 check.
//!
//! Pinning rules (Bozniak's shared-package rules 1 and 2):
//! - every dependency names an id, an **exact** version (`1.4.2`; no `^`, `~`, `>=`, `*`, `x`, `||`, ranges or
//!   build metadata), the sha256 of its content, and the team id of whoever signed it;
//! - a dependency with code is `kind: "xpc-service"`: its own sandboxed process, never a dylib or framework
//!   loaded into the module. It also names its bundle id and the exact designated requirement checked before every
//!   connection;
//! - a dependency without code is `kind: "data"` (a model set): read-only files, no designated requirement;
//! - a module depends on at most one version of a package.
use crate::{ModuleError, MODULE_IDS};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 1;
/// Manifests are small; anything bigger is refused before parsing.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// The team id an unsigned dev build uses. Accepted by the parser so the repo's manifests are valid before the
/// release signing setup exists (docs/release.md); a release launcher refuses it ([`CodeIdentity::is_placeholder`]).
pub const PLACEHOLDER_TEAM_ID: &str = "0000000000";
/// The Developer ID team every release path signs and pins: Jared Grimes's
/// `Developer ID Application: Jared Grimes (MR26CCZ496)`. The bundled manifests, `modules/modules.lock.json`, the
/// confinement map's bundledCode and the release scripts all name it (checked by
/// crates/qrtz-lifecycle/tests/offline_graph.rs); only test fixtures use another team.
pub const RELEASE_TEAM_ID: &str = "MR26CCZ496";
/// Kinds that would put shared code inside a module's process. Named so the error says why.
const IN_PROCESS_KINDS: &[&str] = &["dylib", "framework", "library", "bundle", "plugin", "static"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleManifest {
    pub schema: u32,
    /// `echo`, `nano` or `ctrl`.
    pub id: String,
    /// Shown in settings (lowercase, like all qrtz text).
    pub name: String,
    pub version: String,
    /// The helper protocol version the module speaks (contracts/helper-protocol.json).
    pub protocol: u32,
    pub min_app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_app: Option<String>,
    /// The module's own code identity, checked before each launch.
    pub code: CodeIdentity,
    pub launch: Launch,
    /// Renderer views this module turns on (they stay in the signed shell, ADR 0078 CSP).
    pub views: Vec<String>,
    /// IPC capabilities (packages/core capabilities.ts) the module's views may use.
    pub capabilities: Vec<String>,
    /// TCC permissions the module asks for (the module manager shows them beside its switch; macOS asks on first use).
    pub permissions: Vec<Permission>,
    /// Entitlements the module's signed bundle carries (M5).
    pub entitlements: Vec<String>,
    pub settings: SettingsDecl,
    /// XPC services this module offers to others (none yet; shared code arrives as its own package in phase 2).
    #[serde(default)]
    pub provides: Vec<String>,
    pub dependencies: Vec<Dependency>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeIdentity {
    pub bundle_id: String,
    pub team_id: String,
    /// Must equal [`designated_requirement`] of `bundle_id` and `team_id`.
    pub designated_requirement: String,
}

impl CodeIdentity {
    pub fn is_placeholder(&self) -> bool {
        self.team_id == PLACEHOLDER_TEAM_ID
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchKind {
    /// Today (M0-M5): the helper is a Tauri `externalBin` in qrtz.app, spawned by the host.
    BundledHelper,
    /// ADR 0169: the module's own executable in its installed extension package (`entry` is the package's
    /// `extension.entry`), signed with the module's identifier and spawned by the host. qrtz.app bundles none.
    Package,
    /// M6 on: a signed LSUIElement `.app` opened through LaunchServices so TCC attributes it to the module.
    LaunchServices,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Launch {
    pub kind: LaunchKind,
    /// The executable (bundled helper, package) or `.app` (LaunchServices), relative to the module's install dir.
    pub entry: String,
    /// Helper line prefixes this module owns (`ctrl.*` already has its own queue, ADR 0110).
    #[serde(default)]
    pub channels: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    Microphone,
    Accessibility,
    ScreenRecording,
    InputMonitoring,
    Automation,
    FilesAndFolders,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsDecl {
    /// The module's settings file: `modules/<id>/settings.json` under the app data dir.
    #[serde(default)]
    pub namespace: String,
    /// Today's config.json sections that move into this module's settings ([`crate::settings::split_legacy`]).
    #[serde(default)]
    pub legacy_sections: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKind {
    /// Read-only files (a model set). No code, so no designated requirement.
    Data,
    /// Shared code in its own sandboxed process; reached only over XPC ([`crate::ipc`]).
    XpcService,
}

impl DependencyKind {
    pub fn id_prefix(self) -> &'static str {
        match self {
            Self::Data => "model:",
            Self::XpcService => "xpc:",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dependency {
    /// `model:<name>` for data, `xpc:<name>` for a service.
    pub id: String,
    pub kind: DependencyKind,
    /// Exact version.
    pub version: String,
    /// sha256 of the package's content ([`crate::lock::content_digest`]).
    pub sha256: String,
    /// Team id of the publisher (the Developer ID team for code; the index signer's team for data).
    pub team_id: String,
    /// xpc-service only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// xpc-service only; must equal [`designated_requirement`] of `bundle_id` and `team_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designated_requirement: Option<String>,
    /// The module runs without it (ctrl's Laya model: the heuristic works without it). Still pinned exactly.
    #[serde(default)]
    pub optional: bool,
}

/// The Developer ID designated requirement for `bundle_id` signed by `team_id`. The manifest must carry exactly
/// this string, so a looser requirement (no team, no anchor) can't be smuggled in.
pub fn designated_requirement(bundle_id: &str, team_id: &str) -> String {
    format!(
        "identifier \"{bundle_id}\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = \"{team_id}\""
    )
}

/// An exact version: `MAJOR.MINOR.PATCH` with an optional pre-release, nothing else.
pub fn exact_version(field: &str, v: &str) -> Result<semver::Version, ModuleError> {
    let t = v.trim();
    let range = t != v
        || t.is_empty()
        || t.starts_with(|c: char| "^~=<>*!".contains(c))
        || t.contains(|c: char| c.is_whitespace() || c == ',' || c == '|' || c == '*')
        || t.split('.').any(|p| p.eq_ignore_ascii_case("x"))
        || t.split('.').count() < 3;
    if range {
        return Err(ModuleError::VersionRange(format!("{field} {v:?}")));
    }
    if t.contains('+') {
        return Err(ModuleError::invalid(
            field,
            format!("{v:?}: build metadata is ignored by version order, so it can't pin anything"),
        ));
    }
    semver::Version::parse(t).map_err(|e| ModuleError::invalid(field, format!("{v:?}: {e}")))
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn is_team_id(s: &str) -> bool {
    s.len() == 10 && s.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
}

fn is_bundle_id(s: &str) -> bool {
    s.starts_with("ai.qrtz.")
        && s.len() <= 128
        && s.split('.').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn is_name(s: &str) -> bool {
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

/// Specific errors for the two things the schema exists to refuse, before serde's generic ones.
fn precheck(raw: &Value) -> Result<(), ModuleError> {
    if let Some(deps) = raw.get("dependencies").and_then(Value::as_array) {
        for d in deps {
            let id = d.get("id").and_then(Value::as_str).unwrap_or("?").to_owned();
            if let Some(kind) = d.get("kind").and_then(Value::as_str) {
                if IN_PROCESS_KINDS.contains(&kind) {
                    return Err(ModuleError::InProcessCode {
                        id,
                        kind: kind.to_owned(),
                    });
                }
            }
            if let Some(v) = d.get("version").and_then(Value::as_str) {
                exact_version(&format!("dependencies[{id}].version"), v)?;
            }
            for k in ["range", "versions", "requires", "semver"] {
                if d.get(k).is_some() {
                    return Err(ModuleError::VersionRange(format!("dependencies[{id}].{k}")));
                }
            }
        }
    }
    if raw.get("requires").is_some() {
        return Err(ModuleError::VersionRange("requires".into()));
    }
    Ok(())
}

impl ModuleManifest {
    /// Parse and validate. The only way to get a manifest from bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, ModuleError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ModuleError::invalid("manifest", "too large"));
        }
        let raw: Value = serde_json::from_slice(bytes).map_err(|e| ModuleError::Json(e.to_string()))?;
        precheck(&raw)?;
        let m: Self = serde_json::from_value(raw).map_err(|e| ModuleError::Json(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn semver(&self) -> semver::Version {
        semver::Version::parse(&self.version).expect("validated")
    }

    pub fn validate(&self) -> Result<(), ModuleError> {
        if self.schema != SCHEMA_VERSION {
            return Err(ModuleError::invalid(
                "schema",
                format!("unsupported {}", self.schema),
            ));
        }
        if !MODULE_IDS.contains(&self.id.as_str()) {
            return Err(ModuleError::invalid(
                "id",
                format!("{:?} is not a qrtz module", self.id),
            ));
        }
        if self.name.is_empty() || self.name != self.name.to_lowercase() {
            return Err(ModuleError::invalid("name", "lowercase, not empty"));
        }
        exact_version("version", &self.version)?;
        exact_version("minApp", &self.min_app)?;
        if let Some(max) = &self.max_app {
            if exact_version("maxApp", max)? < exact_version("minApp", &self.min_app)? {
                return Err(ModuleError::invalid("maxApp", "below minApp"));
            }
        }
        let c = &self.code;
        if c.bundle_id != format!("ai.qrtz.{}", self.id) {
            return Err(ModuleError::invalid(
                "code.bundleId",
                format!("must be ai.qrtz.{}", self.id),
            ));
        }
        if !is_team_id(&c.team_id) {
            return Err(ModuleError::invalid(
                "code.teamId",
                "10 uppercase letters or digits",
            ));
        }
        if c.designated_requirement != designated_requirement(&c.bundle_id, &c.team_id) {
            return Err(ModuleError::invalid(
                "code.designatedRequirement",
                "must be the Developer ID requirement for code.bundleId and code.teamId",
            ));
        }
        if !safe_rel(&self.launch.entry) {
            return Err(ModuleError::invalid(
                "launch.entry",
                "a relative path inside the module",
            ));
        }
        for ch in &self.launch.channels {
            if !(ch.ends_with(".*") && is_name(&ch[..ch.len() - 2])) {
                return Err(ModuleError::invalid(
                    "launch.channels",
                    format!("{ch:?}: `<name>.*`"),
                ));
            }
        }
        for v in &self.views {
            if !is_name(v) {
                return Err(ModuleError::invalid("views", format!("{v:?}")));
            }
        }
        if self.settings.namespace != self.id {
            return Err(ModuleError::invalid("settings.namespace", "the module id"));
        }
        for s in &self.provides {
            if !s.strip_prefix("xpc:").is_some_and(is_name) {
                return Err(ModuleError::invalid("provides", format!("{s:?}: `xpc:<name>`")));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for d in &self.dependencies {
            d.validate()?;
            if !seen.insert(d.id.as_str()) {
                return Err(ModuleError::invalid(
                    format!("dependencies[{}]", d.id),
                    "listed twice; a module depends on one version of a package",
                ));
            }
        }
        Ok(())
    }
}

impl Dependency {
    pub fn validate(&self) -> Result<(), ModuleError> {
        let f = |k: &str| format!("dependencies[{}].{k}", self.id);
        let name = self.id.strip_prefix(self.kind.id_prefix());
        if !name.is_some_and(is_name) {
            return Err(ModuleError::invalid(
                f("id"),
                format!(
                    "{:?} dependencies are named `{}<name>`",
                    self.kind,
                    self.kind.id_prefix()
                ),
            ));
        }
        exact_version(&f("version"), &self.version)?;
        if !is_sha256(&self.sha256) {
            return Err(ModuleError::invalid(f("sha256"), "64 lowercase hex"));
        }
        if !is_team_id(&self.team_id) {
            return Err(ModuleError::invalid(
                f("teamId"),
                "10 uppercase letters or digits",
            ));
        }
        match self.kind {
            DependencyKind::Data => {
                if self.bundle_id.is_some() || self.designated_requirement.is_some() {
                    return Err(ModuleError::invalid(
                        f("designatedRequirement"),
                        "data has no code; a package with code must be an xpc-service",
                    ));
                }
            }
            DependencyKind::XpcService => {
                let Some(b) = self.bundle_id.as_deref().filter(|b| is_bundle_id(b)) else {
                    return Err(ModuleError::invalid(
                        f("bundleId"),
                        "required, ai.qrtz.inference.<name>",
                    ));
                };
                let want = format!("ai.qrtz.inference.{}", name.unwrap_or_default());
                if b != want {
                    return Err(ModuleError::invalid(f("bundleId"), format!("must be {want}")));
                }
                if self.designated_requirement.as_deref() != Some(&designated_requirement(b, &self.team_id)) {
                    return Err(ModuleError::invalid(
                        f("designatedRequirement"),
                        "must be the Developer ID requirement for bundleId and teamId",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn semver(&self) -> semver::Version {
        semver::Version::parse(&self.version).expect("validated")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_versions_only() {
        for ok in ["1.0.0", "0.17.7", "2.0.0-rc.1"] {
            exact_version("v", ok).unwrap();
        }
        for range in [
            "^1.4",
            "~1.4.0",
            ">=1.0.0",
            "=1.0.0",
            "1.x",
            "1.2.X",
            "*",
            "1.0.0 || 2.0.0",
            "1.0.0, 2.0.0",
            "1.4",
            "1",
            "",
            " 1.0.0",
            "<2.0.0",
            "!1.0.0",
        ] {
            assert!(
                matches!(exact_version("v", range), Err(ModuleError::VersionRange(_))),
                "{range:?} accepted"
            );
        }
        assert!(matches!(
            exact_version("v", "1.0.0+abc"),
            Err(ModuleError::Invalid { .. })
        ));
        assert!(matches!(
            exact_version("v", "1.0.0.0"),
            Err(ModuleError::Invalid { .. })
        ));
    }

    #[test]
    fn the_requirement_pins_identifier_anchor_and_team() {
        let dr = designated_requirement("ai.qrtz.echo", "ABCDE12345");
        assert!(dr.starts_with("identifier \"ai.qrtz.echo\" and anchor apple generic"));
        assert!(dr.ends_with("certificate leaf[subject.OU] = \"ABCDE12345\""));
    }
}
