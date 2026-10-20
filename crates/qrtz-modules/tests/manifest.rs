#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0146 manifest rules: exact pins, code only as xpc-service, requirement text pinned.
mod common;
use common::*;
use qrtz_modules::manifest::ModuleManifest;
use qrtz_modules::ModuleError;
use serde_json::{json, Value};

fn parse(v: &Value) -> Result<ModuleManifest, ModuleError> {
    ModuleManifest::parse(v.to_string().as_bytes())
}

#[test]
fn a_manifest_with_exact_data_and_xpc_pins_parses() {
    let m = manifest(
        "echo",
        "1.2.3",
        vec![data_dep("parakeet-tdt-v2", "2.0.0"), xpc_dep("voice", "1.4.0")],
    );
    assert_eq!(m.dependencies.len(), 2);
    assert!(is_xpc(&m.dependencies[1]));
}

#[test]
fn a_semver_range_anywhere_is_refused() {
    for r in ["^1.4", "~1.4.0", ">=1.4.0", "1.x", "*", "1.4.0 || 2.0.0", "1.4"] {
        let mut d = xpc_dep("voice", "1.4.0");
        d["version"] = json!(r);
        let e = parse(&manifest_json("echo", "1.0.0", vec![d])).unwrap_err();
        assert!(matches!(e, ModuleError::VersionRange(_)), "{r}: {e}");
    }
    let mut v = manifest_json("echo", "1.0.0", vec![]);
    v["version"] = json!("^1.0.0");
    assert!(matches!(parse(&v), Err(ModuleError::VersionRange(_))));
    // An npm-style `requires` map is the proposal's first draft; it is not accepted.
    let mut v = manifest_json("echo", "1.0.0", vec![]);
    v["requires"] = json!({"xpc:voice": "^1.4"});
    assert!(matches!(parse(&v), Err(ModuleError::VersionRange(_))));
}

#[test]
fn in_process_code_is_refused_by_name() {
    for kind in ["dylib", "framework", "library", "bundle", "plugin", "static"] {
        let mut d = xpc_dep("voice", "1.4.0");
        d["kind"] = json!(kind);
        let e = parse(&manifest_json("nano", "1.0.0", vec![d])).unwrap_err();
        assert!(matches!(e, ModuleError::InProcessCode { .. }), "{kind}: {e}");
    }
    let mut d = xpc_dep("voice", "1.4.0");
    d["kind"] = json!("something-else");
    assert!(parse(&manifest_json("nano", "1.0.0", vec![d])).is_err());
}

#[test]
fn an_xpc_service_must_pin_bundle_id_team_and_the_exact_requirement() {
    let base = xpc_dep("voice", "1.4.0");
    let mut cases = vec![];
    for k in ["bundleId", "designatedRequirement", "teamId", "sha256"] {
        let mut d = base.clone();
        d.as_object_mut().unwrap().remove(k);
        cases.push((format!("no {k}"), d));
    }
    let mut d = base.clone();
    d["designatedRequirement"] = json!("identifier \"ai.qrtz.inference.voice\"");
    cases.push(("loose requirement".into(), d));
    let mut d = base.clone();
    d["designatedRequirement"] = json!(qrtz_modules::manifest::designated_requirement(
        "ai.qrtz.inference.voice",
        "ZZZZZ99999"
    ));
    cases.push(("requirement for another team".into(), d));
    let mut d = base.clone();
    d["bundleId"] = json!("ai.qrtz.echo");
    cases.push(("bundle id of a module".into(), d));
    let mut d = base.clone();
    d["sha256"] = json!("ABC");
    cases.push(("bad sha".into(), d));
    let mut d = base.clone();
    d["teamId"] = json!("abcde12345");
    cases.push(("lowercase team".into(), d));
    let mut d = base;
    d["id"] = json!("model:voice");
    cases.push(("xpc with a model id".into(), d));
    for (what, d) in cases {
        assert!(
            parse(&manifest_json("echo", "1.0.0", vec![d])).is_err(),
            "{what} accepted"
        );
    }
}

#[test]
fn data_carries_no_requirement_and_one_version_per_package() {
    let mut d = data_dep("laya", "1.0.0");
    d["designatedRequirement"] = json!("x");
    assert!(parse(&manifest_json("ctrl", "1.0.0", vec![d])).is_err());
    let twice = vec![data_dep("laya", "1.0.0"), data_dep("laya", "1.1.0")];
    assert!(parse(&manifest_json("ctrl", "1.0.0", twice)).is_err());
}

#[test]
fn the_module_itself_is_pinned_to_its_bundle_id_and_requirement() {
    let mut v = manifest_json("echo", "1.0.0", vec![]);
    v["id"] = json!("other");
    assert!(parse(&v).is_err(), "unknown module");
    let mut v = manifest_json("echo", "1.0.0", vec![]);
    v["code"]["bundleId"] = json!("ai.qrtz.nano");
    assert!(parse(&v).is_err(), "another module's bundle id");
    let mut v = manifest_json("echo", "1.0.0", vec![]);
    v["code"]["designatedRequirement"] = json!("anchor apple generic");
    assert!(parse(&v).is_err(), "loose requirement");
    let mut v = manifest_json("echo", "1.0.0", vec![]);
    v["launch"]["entry"] = json!("../../bin/sh");
    assert!(parse(&v).is_err(), "entry outside the module");
    let mut v = manifest_json("echo", "1.0.0", vec![]);
    v["surprise"] = json!(1);
    assert!(parse(&v).is_err(), "unknown field");
    let mut v = manifest_json("echo", "1.0.0", vec![]);
    v["name"] = json!("Echo");
    assert!(parse(&v).is_err(), "uppercase name");
}
