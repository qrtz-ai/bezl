#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0146 lockfile: lookup-only resolution, side by side versions, no silent change, all or nothing.
mod common;
use common::*;
use qrtz_modules::lock::{LockedPackage, Lockfile, PinnedRef, Source};
use qrtz_modules::{sha256_hex, ModuleError};

#[test]
fn resolution_is_a_lookup_in_the_lockfile_only() {
    let echo = manifest(
        "echo",
        "1.0.0",
        vec![xpc_dep("voice", "1.4.0"), data_dep("parakeet-tdt-v2", "2.0.0")],
    );
    // Nothing installed: missing, and there is nowhere else to look.
    assert_eq!(
        Lockfile::default().resolve(&echo).unwrap_err(),
        ModuleError::Missing {
            id: "xpc:voice".into(),
            version: "1.4.0".into()
        }
    );
    let lock = install(&Lockfile::default(), &echo);
    let r = lock.resolve(&echo).unwrap();
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|r| r.package.is_some()));
    // A manifest pinning 1.4.1 doesn't pick up the installed 1.4.0 (no ranges, no "compatible" match).
    let newer = manifest("echo", "1.0.1", vec![xpc_dep("voice", "1.4.1")]);
    assert!(matches!(lock.resolve(&newer), Err(ModuleError::Missing { .. })));
}

#[test]
fn every_pinned_field_must_match_the_installed_package() {
    let echo = manifest("echo", "1.0.0", vec![xpc_dep("voice", "1.4.0")]);
    let lock = install(&Lockfile::default(), &echo);
    type Edit = fn(&mut LockedPackage);
    let fields: [(&str, Edit); 3] = [
        ("sha256", |p| p.sha256 = hash("other")),
        ("teamId", |p| p.team_id = "ZZZZZ99999".into()),
        ("designatedRequirement", |p| {
            p.designated_requirement = Some("anchor apple".into())
        }),
    ];
    for (field, f) in fields {
        let mut l = lock.clone();
        f(l.packages.get_mut("xpc:voice").unwrap().get_mut("1.4.0").unwrap());
        match l.resolve(&echo) {
            Err(ModuleError::Mismatch { field: got, .. }) => assert_eq!(got, field),
            other => panic!("{field}: {other:?}"),
        }
    }
}

#[test]
fn two_modules_pinning_different_versions_get_both_side_by_side() {
    let echo = manifest("echo", "1.0.0", vec![xpc_dep("voice", "1.4.0")]);
    let nano = manifest("nano", "1.0.0", vec![xpc_dep("voice", "2.0.0")]);
    let lock = install(&install(&Lockfile::default(), &echo), &nano);
    let voice = &lock.packages["xpc:voice"];
    assert_eq!(voice.keys().collect::<Vec<_>>(), ["1.4.0", "2.0.0"]);
    assert_eq!(voice["1.4.0"].path, "packages/xpc/voice/1.4.0");
    assert_eq!(lock.required_by("xpc:voice", "1.4.0"), ["echo"]);
    assert_eq!(lock.required_by("xpc:voice", "2.0.0"), ["nano"]);
    // echo still resolves to exactly what it pinned.
    let r = lock.resolve(&echo).unwrap();
    assert_eq!(r[0].dependency.version, "1.4.0");
}

#[test]
fn an_installed_version_never_changes_content_underneath_a_module() {
    let echo = manifest("echo", "1.0.0", vec![xpc_dep("voice", "1.4.0")]);
    let lock = install(&Lockfile::default(), &echo);
    // nano arrives pinning the same id and version but different bytes.
    let mut d = xpc_dep("voice", "1.4.0");
    d["sha256"] = serde_json::json!(hash("evil"));
    let nano = manifest("nano", "1.0.0", vec![d]);
    let e = lock
        .plan_install(&nano, &sha256_hex(&bytes(&nano)), Source::Network, &brings(&nano))
        .unwrap_err();
    assert_eq!(
        e,
        ModuleError::ContentChanged {
            id: "xpc:voice".into(),
            version: "1.4.0".into()
        }
    );
}

#[test]
fn an_install_brings_only_what_it_pins_and_everything_it_needs() {
    let echo = manifest("echo", "1.0.0", vec![xpc_dep("voice", "1.4.0")]);
    let other = manifest("nano", "1.0.0", vec![xpc_dep("voice", "9.0.0")]);
    let mut extra = brings(&echo);
    extra.extend(brings(&other));
    let sha = sha256_hex(&bytes(&echo));
    assert!(matches!(
        Lockfile::default().plan_install(&echo, &sha, Source::Network, &extra),
        Err(ModuleError::Lock(_))
    ));
    assert!(matches!(
        Lockfile::default().plan_install(&echo, &sha, Source::Network, &[]),
        Err(ModuleError::Missing { .. })
    ));
    // The pin and the package brought must agree.
    let mut wrong = brings(&echo);
    wrong[0].1.sha256 = hash("x");
    assert!(matches!(
        Lockfile::default().plan_install(&echo, &sha, Source::Network, &wrong),
        Err(ModuleError::Mismatch { .. })
    ));
}

#[test]
fn an_optional_dependency_may_be_absent_but_never_floats() {
    let mut d = data_dep("laya", "1.0.0");
    d["optional"] = serde_json::json!(true);
    let ctrl = manifest("ctrl", "1.0.0", vec![d]);
    let lock = Lockfile::default()
        .plan_install(&ctrl, &sha256_hex(&bytes(&ctrl)), Source::Bundled, &[])
        .unwrap();
    assert!(lock.modules["ctrl"].dependencies.is_empty());
    assert!(lock.resolve(&ctrl).unwrap()[0].package.is_none());
}

#[test]
fn removing_a_module_frees_only_what_nothing_else_pins() {
    let echo = manifest(
        "echo",
        "1.0.0",
        vec![xpc_dep("voice", "1.4.0"), data_dep("parakeet-tdt-v2", "2.0.0")],
    );
    let nano = manifest("nano", "1.0.0", vec![xpc_dep("voice", "1.4.0")]);
    let lock = install(&install(&Lockfile::default(), &echo), &nano);
    assert!(lock.unreferenced().is_empty());
    let (after, gone) = lock.plan_remove("echo").collect_garbage();
    assert_eq!(
        gone,
        [PinnedRef {
            id: "model:parakeet-tdt-v2".into(),
            version: "2.0.0".into()
        }]
    );
    assert!(after.package("xpc:voice", "1.4.0").is_some());
    after.validate().unwrap();
}

#[test]
fn an_upgrade_leaves_other_modules_pins_alone() {
    let echo1 = manifest("echo", "1.0.0", vec![xpc_dep("voice", "1.4.0")]);
    let nano = manifest("nano", "1.0.0", vec![xpc_dep("voice", "1.4.0")]);
    let lock = install(&install(&Lockfile::default(), &echo1), &nano);
    let echo2 = manifest("echo", "1.1.0", vec![xpc_dep("voice", "1.5.0")]);
    let lock = install(&lock, &echo2);
    assert_eq!(lock.modules["nano"].dependencies[0].version, "1.4.0");
    assert_eq!(lock.modules["echo"].dependencies[0].version, "1.5.0");
    assert!(lock.resolve(&nano).is_ok());
}

#[test]
fn the_lockfile_round_trips_atomically_and_a_corrupt_one_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("modules.lock.json");
    assert_eq!(Lockfile::load(&p).unwrap(), Lockfile::default());
    let echo = manifest("echo", "1.0.0", vec![xpc_dep("voice", "1.4.0")]);
    let lock = install(&Lockfile::default(), &echo);
    lock.save(&p).unwrap();
    assert_eq!(Lockfile::load(&p).unwrap(), lock);
    assert_eq!(std::fs::read(&p).unwrap(), lock.to_bytes(), "deterministic bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(!dir.path().join(".modules.lock.json.tmp").exists());
    std::fs::write(&p, b"{not json").unwrap();
    assert!(Lockfile::load(&p).is_err());
    // A lockfile whose module pins a package it doesn't list is refused.
    let mut bad = lock;
    bad.packages.clear();
    assert!(Lockfile::parse(&bad.to_bytes()).is_err());
}
