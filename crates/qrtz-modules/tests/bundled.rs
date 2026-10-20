#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
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
        // snap (ADR 0159) has no dependencies to bend.
        if v["dependencies"].as_array().is_none_or(|d| d.is_empty()) {
            continue;
        }
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

/// What `id` owns in the confinement map that really exists under `root`: a crate (a Cargo.toml under crates/,
/// tools/ or apps/ whose package has that name) or a Swift target (a `.target` or `.executableTarget` of that name
/// in native/qrtz-helper/Package.swift, not a test target, with at least one .swift file in Sources/<name>). A map
/// entry alone is not enough, so no module can skip owning real code by naming a target that isn't there.
fn owned_code(map: &Value, id: &str, root: &Path) -> Vec<String> {
    let owned_in = |k: &str| -> Vec<String> {
        map[k]
            .as_object()
            .map(|o| {
                o.iter()
                    .filter(|(_, v)| *v == id)
                    .map(|(n, _)| n.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut cargo_names = Vec::new();
    for base in ["crates", "tools", "apps"] {
        let Ok(rd) = std::fs::read_dir(root.join(base)) else {
            continue;
        };
        for e in rd.flatten() {
            for toml in [e.path().join("Cargo.toml"), e.path().join("src-tauri/Cargo.toml")] {
                if let Ok(t) = std::fs::read_to_string(&toml) {
                    if let Some(pkg) = t.split("[package]").nth(1) {
                        let pkg = pkg.split("\n[").next().unwrap_or("");
                        if let Some(n) = pkg.lines().find_map(|l| {
                            let l = l.trim();
                            l.strip_prefix("name")
                                .map(|r| r.trim_start())
                                .and_then(|r| r.strip_prefix('='))
                                .map(|r| r.trim().trim_matches('"').to_owned())
                        }) {
                            cargo_names.push(n);
                        }
                    }
                }
            }
        }
    }
    let pkg_dir = root.join("native/qrtz-helper");
    let swift: String = std::fs::read_to_string(pkg_dir.join("Package.swift"))
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    fn has_swift(d: &Path) -> bool {
        std::fs::read_dir(d).is_ok_and(|rd| {
            rd.flatten().any(|e| {
                let p = e.path();
                if p.is_dir() {
                    has_swift(&p)
                } else {
                    p.extension().is_some_and(|x| x == "swift")
                }
            })
        })
    }
    let mut out: Vec<String> = owned_in("crates")
        .into_iter()
        .filter(|c| cargo_names.contains(c))
        .map(|c| format!("crate {c}"))
        .collect();
    out.extend(
        owned_in("swiftTargets")
            .into_iter()
            .filter(|t| {
                let declared = [".target(name:", ".executableTarget(name:"]
                    .iter()
                    .any(|k| swift.contains(&format!("{k}\"{t}\"")));
                declared && has_swift(&pkg_dir.join("Sources").join(t))
            })
            .map(|t| format!("swift target {t}")),
    );
    out
}

/// The owner check above can't be satisfied by a map entry alone: a module needs a crate or Swift target that
/// exists, and a Swift target only counts with sources and as a real (non-test) target.
#[test]
fn a_module_owns_code_only_when_the_crate_or_swift_target_exists() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let pkg = r.join("native/qrtz-helper");
    std::fs::create_dir_all(pkg.join("Sources/Ghost")).unwrap();
    std::fs::create_dir_all(pkg.join("Sources/GhostTests")).unwrap();
    std::fs::write(pkg.join("Sources/GhostTests/a.swift"), "").unwrap();
    let map = serde_json::json!({
        "crates": { "ghost-crate": "ghost" },
        "swiftTargets": { "Ghost": "ghost", "GhostTests": "ghost", "Missing": "ghost" }
    });
    // Neither the crate nor any target exists yet.
    std::fs::write(
        pkg.join("Package.swift"),
        "targets: [ .testTarget(name: \"GhostTests\") ]",
    )
    .unwrap();
    assert!(owned_code(&map, "ghost", r).is_empty());
    // A module with no entries owns nothing.
    assert!(owned_code(&map, "nobody", r).is_empty());
    // Declared but without sources: still nothing.
    std::fs::write(
        pkg.join("Package.swift"),
        "targets: [ .target(name: \"Ghost\"), .testTarget(name: \"GhostTests\") ]",
    )
    .unwrap();
    assert!(owned_code(&map, "ghost", r).is_empty());
    // With a source file it counts (a test target with sources never does).
    std::fs::create_dir_all(pkg.join("Sources/Ghost/mac")).unwrap();
    std::fs::write(pkg.join("Sources/Ghost/mac/a.swift"), "").unwrap();
    assert_eq!(
        owned_code(&map, "ghost", r),
        vec!["swift target Ghost".to_string()]
    );
    // A crate counts once its Cargo.toml names it.
    std::fs::create_dir_all(r.join("crates/ghost-crate")).unwrap();
    std::fs::write(
        r.join("crates/ghost-crate/Cargo.toml"),
        "[package]\nname = \"ghost-crate\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    assert_eq!(owned_code(&map, "ghost", r).len(), 2);
}

/// The ADR 0144 map is the stand-in owner list until M1; the manifests already agree with it on what each module
/// owns, and the shell's own crate is the shell's.
#[test]
fn the_manifests_agree_with_the_confinement_map() {
    let map = read_json("docs/modules/confinement-map.json");
    assert_eq!(map["crates"]["qrtz-modules"], "shell");
    for m in manifests() {
        // A module owns a Rust crate or (snap, ADR 0159: Swift only) a Swift target, and what it owns must exist.
        let owned = owned_code(&map, &m.id, &root());
        assert!(
            !owned.is_empty(),
            "{} owns no existing crate or Swift target (a crate with a Cargo.toml, or a target or executableTarget \
             in native/qrtz-helper/Package.swift with .swift sources)",
            m.id
        );
        // ADR 0154: one qrtz identity, so a module may declare only the app's TCC allowlist.
        let allowed = map["appTccEntitlements"].as_object().unwrap();
        for e in &m.entitlements {
            assert!(
                allowed.contains_key(e.as_str()),
                "{}: {e} is not in appTccEntitlements (ADR 0154)",
                m.id
            );
        }
    }
}

/// ADR 0146 M1.3: every extension helper exists as a Swift executable, and its probe mode takes exactly the
/// launcher's flag and probes (what scripts/modules/mac-gate.sh drives). ADR 0154: the helpers share qrtz's TCC
/// identity, but entitlements are per process, so a helper that opens the mic (echo, nano) or sends Apple Events
/// (ctrl) carries that one entitlement in its own signature (`entitlements/ai.qrtz.<id>.entitlements`; the
/// offline_graph lint holds it to appTccEntitlements).
#[test]
fn every_extension_helper_has_a_target_its_per_process_entitlement_and_the_launchers_probe_mode() {
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
        let needs = match *id {
            // ADR 0159: snap (and, since the ADR 0214 review, ctrl: Accessibility only, no Apple Events) needs Accessibility only (a TCC grant, not an entitlement) and Carbon hotkeys (none),
            // so its signature carries no entitlement at all.
            "snap" | "ctrl" => {
                let ent =
                    std::fs::read_to_string(pkg.join(format!("entitlements/ai.qrtz.{id}.entitlements")))
                        .unwrap();
                assert!(
                    !ent.contains("<key>"),
                    "qrtz-{id}'s signature carries no entitlement"
                );
                continue;
            }
            _ => "com.apple.security.device.audio-input",
        };
        let ent = std::fs::read_to_string(pkg.join(format!("entitlements/ai.qrtz.{id}.entitlements")))
            .unwrap_or_default();
        assert!(
            ent.contains(&format!("<key>{needs}</key>")),
            "qrtz-{id}'s own signature needs {needs}"
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
