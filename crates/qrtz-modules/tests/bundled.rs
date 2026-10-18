//! The manifests and lockfile in `modules/` (ADR 0146 §2): valid, consistent with the pinned model manifests,
//! matching the JSON Schemas in contracts/, and owning every config.json section exactly once.
mod common;
use qrtz_modules::lock::{content_digest, Lockfile};
use qrtz_modules::manifest::{DependencyKind, ModuleManifest};
use qrtz_modules::settings::{split_legacy, GLOBAL_SECTIONS};
use qrtz_modules::{bundled, sha256_hex};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(root().join(rel)).unwrap()).unwrap()
}

fn manifests() -> Vec<ModuleManifest> {
    bundled::manifests()
        .iter()
        .map(|(id, s)| {
            let m = ModuleManifest::parse(s.as_bytes()).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(m.id, *id);
            m
        })
        .collect()
}

#[test]
fn the_bundled_manifests_and_lockfile_agree() {
    let lock = Lockfile::parse(bundled::LOCK.as_bytes()).unwrap();
    for ((id, src), m) in bundled::manifests().iter().zip(manifests()) {
        let lm = &lock.modules[*id];
        assert_eq!(
            lm.manifest_sha256,
            sha256_hex(src.as_bytes()),
            "{id}: regenerate modules.lock.json"
        );
        assert_eq!(lm.version, m.version);
        assert_eq!(lm.code, m.code);
        let resolved = lock.resolve(&m).unwrap();
        assert!(resolved.iter().all(|r| r.package.is_some()), "{id}");
        // M0 to M2: no shared code yet (FluidAudio is still in the one helper); models are the only packages.
        assert!(
            m.dependencies.iter().all(|d| d.kind == DependencyKind::Data),
            "{id}"
        );
    }
    assert!(lock.unreferenced().is_empty());
}

/// Each `model:` package hash is the content digest of its set in the pinned model manifest, so the two can't drift.
#[test]
fn model_package_hashes_are_the_pinned_model_sets() {
    let lock = Lockfile::parse(bundled::LOCK.as_bytes()).unwrap();
    let mut sets = vec![];
    for f in ["local_models.json", "ctrl_laya_model.json"] {
        let v = read_json(&format!("crates/qrtz-app/src/{f}"));
        sets.extend(v["sets"].as_array().unwrap().clone());
    }
    for (id, versions) in &lock.packages {
        for p in versions.values() {
            let dir = p
                .path
                .strip_prefix("models/")
                .unwrap_or_else(|| panic!("{id}: {}", p.path));
            let set = sets
                .iter()
                .find(|s| s["dir"] == dir)
                .unwrap_or_else(|| panic!("{id}: no model set with dir {dir}"));
            let files = set["files"].as_array().unwrap();
            let digest = content_digest(files.iter().map(|f| {
                (
                    f["path"].as_str().unwrap(),
                    f["size"].as_u64().unwrap(),
                    f["sha256"].as_str().unwrap(),
                )
            }));
            assert_eq!(
                p.sha256, digest,
                "{id}: model set {dir} changed; update modules/ and the lockfile"
            );
        }
    }
    let used: usize = lock.packages.values().map(|v| v.len()).sum();
    assert_eq!(used, sets.len(), "every pinned model set belongs to a module");
}

#[test]
fn the_json_schemas_accept_the_bundled_files_and_refuse_ranges_and_dylibs() {
    let ms = jsonschema::validator_for(&read_json("contracts/module-manifest.schema.json")).unwrap();
    let ls = jsonschema::validator_for(&read_json("contracts/modules-lock.schema.json")).unwrap();
    for (id, src) in bundled::manifests() {
        let v: Value = serde_json::from_str(src).unwrap();
        let errs: Vec<String> = ms.iter_errors(&v).map(|e| e.to_string()).collect();
        assert!(errs.is_empty(), "{id}: {errs:?}");
        let mut range = v.clone();
        range["dependencies"][0]["version"] = "^1.0.0".into();
        assert!(!ms.is_valid(&range), "{id}: range accepted");
        let mut dylib = v.clone();
        dylib["dependencies"][0]["kind"] = "dylib".into();
        assert!(!ms.is_valid(&dylib), "{id}: dylib accepted");
    }
    let lock: Value = serde_json::from_str(bundled::LOCK).unwrap();
    assert!(ls.is_valid(&lock));
    // Rust and the schema agree on a synthetic xpc-service pin.
    let m = common::manifest_json("echo", "1.0.0", vec![common::xpc_dep("voice", "1.4.0")]);
    assert!(ms.is_valid(&m));
    ModuleManifest::parse(m.to_string().as_bytes()).unwrap();
}

/// Every config.json section is either global or one module's (Jared: module settings live in the module).
#[test]
fn every_config_section_has_one_owner() {
    let contract = read_json("contracts/ipc-contract.json");
    let props = contract["config"]["schema"]["properties"].as_object().unwrap();
    let cfg: Value = props
        .keys()
        .map(|k| (k.clone(), Value::Null))
        .collect::<serde_json::Map<_, _>>()
        .into();
    let ms = manifests();
    let split = split_legacy(&cfg, &ms);
    assert!(
        split.unowned.is_empty(),
        "unowned config sections: {:?}",
        split.unowned
    );
    let mut owned: Vec<&String> = ms.iter().flat_map(|m| &m.settings.legacy_sections).collect();
    owned.sort();
    let before = owned.len();
    owned.dedup();
    assert_eq!(before, owned.len(), "a section owned twice");
    assert!(owned.iter().all(|s| !GLOBAL_SECTIONS.contains(&s.as_str())));
    assert!(
        props.contains_key("modules"),
        "config has the global module switches"
    );
}

/// The ADR 0144 map is the stand-in owner list until M1; the manifests already agree with it on what each module
/// owns, and the shell's own crate is the shell's.
#[test]
fn the_manifests_agree_with_the_confinement_map() {
    let map = read_json("docs/modules/confinement-map.json");
    assert_eq!(map["crates"]["qrtz-modules"], "shell");
    for m in manifests() {
        let owns = map["crates"]
            .as_object()
            .unwrap()
            .values()
            .any(|o| o == m.id.as_str());
        assert!(owns, "{} owns no crate in the map", m.id);
        let allowed = &map["entitlements"]["allowedTrue"][&m.id];
        for e in &m.entitlements {
            assert!(
                allowed.as_array().unwrap().iter().any(|a| a == e),
                "{}: {e} is not in ADR 0144's row",
                m.id
            );
        }
    }
}

/// ADR 0146 M1.3: every module app exists as a Swift target with its own entitlements file, and its probe mode
/// takes exactly the launcher's flag and probes (what scripts/modules/mac-gate.sh drives).
#[test]
fn every_module_has_an_app_target_entitlements_and_the_launchers_probe_mode() {
    use qrtz_modules::launcher::{GATE_PROBES, GATE_PROBE_FLAG};
    let pkg = root().join("native/qrtz-helper");
    let probe = std::fs::read_to_string(pkg.join("Sources/QrtzGateProbe/GateProbe.swift")).unwrap();
    assert!(probe.contains(&format!("public let gateProbeFlag = \"{GATE_PROBE_FLAG}\"")));
    assert!(
        probe.contains(&format!("case {}\n", GATE_PROBES.join(", "))),
        "GateProbeCommand cases"
    );
    let manifest = std::fs::read_to_string(pkg.join("Package.swift")).unwrap();
    for id in qrtz_modules::MODULE_IDS {
        let main = std::fs::read_to_string(pkg.join(format!("Sources/qrtz-{id}/main.swift"))).unwrap();
        assert!(
            main.contains(&format!("GateProbe.runIfRequested(module: \"{id}\")")),
            "qrtz-{id} probe mode"
        );
        assert!(
            manifest.contains(&format!(".executable(name: \"qrtz-{id}\"")),
            "qrtz-{id} product"
        );
        assert!(
            pkg.join(format!("entitlements/ai.qrtz.{id}.entitlements"))
                .is_file(),
            "{id} entitlements"
        );
    }
}

/// ADR 0146 M1.4: a module's capabilities are its view's (packages/features/*/src/manifest.ts, as generated into
/// contracts/ipc-contract.json), so the gate and the router agree on what a module may call.
#[test]
fn module_capabilities_are_their_views_capabilities() {
    let c: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("contracts/ipc-contract.json")).unwrap())
            .unwrap();
    for (id, src) in bundled::manifests() {
        let m = ModuleManifest::parse(src.as_bytes()).unwrap();
        for view in &m.views {
            let caps: Vec<String> = c["manifests"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["id"] == view.as_str())
                .unwrap_or_else(|| panic!("{id}: no manifest for view {view}"))["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            let mut a = caps.clone();
            let mut b = m.capabilities.clone();
            a.sort();
            b.sort();
            assert_eq!(a, b, "{id} vs its view {view}");
        }
    }
}
