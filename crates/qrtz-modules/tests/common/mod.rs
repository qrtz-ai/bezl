#![allow(dead_code)]
use qrtz_modules::lock::{LockedPackage, Lockfile, PinnedRef, Source};
use qrtz_modules::manifest::{designated_requirement, Dependency, DependencyKind, ModuleManifest};
use qrtz_modules::sha256_hex;
use qrtz_modules::sideload::{
    Candidate, PackageChecks, RevocationSnapshot, RevocationVerifier, VerifiedSnapshot,
};
use serde_json::{json, Value};

pub const TEAM: &str = "ABCDE12345";
pub const DAY: u64 = 86_400_000;

pub fn hash(s: &str) -> String {
    sha256_hex(s.as_bytes())
}

pub fn data_dep(name: &str, version: &str) -> Value {
    json!({"id": format!("model:{name}"), "kind": "data", "version": version,
           "sha256": hash(&format!("{name}@{version}")), "teamId": TEAM})
}

pub fn xpc_dep(name: &str, version: &str) -> Value {
    let b = format!("ai.qrtz.inference.{name}");
    json!({"id": format!("xpc:{name}"), "kind": "xpc-service", "version": version,
           "sha256": hash(&format!("{name}@{version}")), "teamId": TEAM,
           "bundleId": b, "designatedRequirement": designated_requirement(&b, TEAM)})
}

pub fn manifest_json(id: &str, version: &str, deps: Vec<Value>) -> Value {
    let b = format!("ai.qrtz.{id}");
    json!({
        "schema": 1, "id": id, "name": id, "version": version, "protocol": 1, "minApp": "0.2.0",
        "code": {"bundleId": b, "teamId": TEAM, "designatedRequirement": designated_requirement(&b, TEAM)},
        "launch": {"kind": "bundled-helper", "entry": "qrtz-helper", "channels": [format!("{id}.*")]},
        "views": [id], "capabilities": [], "permissions": [], "entitlements": [],
        "settings": {"namespace": id, "legacySections": []},
        "dependencies": deps,
    })
}

pub fn manifest(id: &str, version: &str, deps: Vec<Value>) -> ModuleManifest {
    ModuleManifest::parse(manifest_json(id, version, deps).to_string().as_bytes()).unwrap()
}

pub fn bytes(m: &ModuleManifest) -> Vec<u8> {
    serde_json::to_vec(m).unwrap()
}

/// The packages a manifest pins, at their default paths (what a .qrtzpkg or download brings).
pub fn brings(m: &ModuleManifest) -> Vec<(PinnedRef, LockedPackage)> {
    m.dependencies
        .iter()
        .map(|d| (pin(d), LockedPackage::for_dependency(d)))
        .collect()
}

pub fn pin(d: &Dependency) -> PinnedRef {
    PinnedRef {
        id: d.id.clone(),
        version: d.version.clone(),
    }
}

pub fn install(lock: &Lockfile, m: &ModuleManifest) -> Lockfile {
    lock.plan_install(m, &sha256_hex(&bytes(m)), Source::Network, &brings(m))
        .unwrap()
}

pub fn candidate(m: &ModuleManifest, file: &str) -> Candidate {
    Candidate {
        manifest: m.clone(),
        manifest_sha256: sha256_hex(&bytes(m)),
        package_sha256: hash(file),
        packages: brings(m),
    }
}

pub fn is_xpc(d: &Dependency) -> bool {
    d.kind == DependencyKind::XpcService
}

pub struct Checks(pub Result<(), String>);
impl PackageChecks for Checks {
    fn check(&self, _: &Candidate) -> Result<(), String> {
        self.0.clone()
    }
}
pub const PASS: Checks = Checks(Ok(()));

/// Accepts `sig == b"ok"` and parses the bytes as the snapshot.
pub struct FakeVerifier;
impl RevocationVerifier for FakeVerifier {
    fn verify(&self, bytes: &[u8], sig: &[u8]) -> Result<RevocationSnapshot, String> {
        if sig != b"ok" {
            return Err("bad signature".into());
        }
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }
}

pub fn snapshot(version: u64, issued_ms: u64, revoked: Value) -> RevocationSnapshot {
    serde_json::from_value(
        json!({"version": version, "issuedMs": issued_ms, "expiresMs": issued_ms + 8 * DAY,
                                  "revoked": revoked}),
    )
    .unwrap()
}

pub fn verified(s: &RevocationSnapshot) -> VerifiedSnapshot {
    VerifiedSnapshot::verify(&FakeVerifier, &serde_json::to_vec(s).unwrap(), b"ok").unwrap()
}
