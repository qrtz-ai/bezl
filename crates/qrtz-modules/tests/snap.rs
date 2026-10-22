//! ADR 0159: qrtz snap's package. Its bundled manifest, its `qrtz.json` (what qrtz-publish packs into the index) and
//! the lockfile agree; it is an extension qrtz knows; it uses only qrtz's Accessibility grant and no entitlement, and
//! depends on nothing (no model, no service, no network).
use qrtz_modules::index::PackageKind;
use qrtz_modules::lock::Lockfile;
use qrtz_modules::qrtz_json::{identifier_for, kind_of, PackageManifest};
use qrtz_modules::{bundled, sha256_hex, ModuleManifest};

fn qrtz_json() -> PackageManifest {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules/snap/qrtz.json");
    PackageManifest::parse(&std::fs::read(p).unwrap()).unwrap()
}

#[test]
fn snap_is_an_extension_qrtz_knows_signed_as_ai_qrtz_snap() {
    assert!(qrtz_modules::MODULE_IDS.contains(&"snap"));
    assert_eq!(kind_of("snap").unwrap(), PackageKind::Extension);
    assert_eq!(
        identifier_for("snap", PackageKind::Extension).as_deref(),
        Some("ai.qrtz.snap")
    );
}

#[test]
fn the_bundled_manifest_qrtz_json_and_lock_agree() {
    let m = ModuleManifest::parse(bundled::SNAP.as_bytes()).unwrap();
    let q = qrtz_json();
    let ext = q.extension.as_ref().expect("an extension section");
    assert_eq!(q.name, m.id);
    assert_eq!(q.version, m.version);
    assert_eq!(q.kind, PackageKind::Extension);
    // ADR 0169: the module's helper is its package's own executable.
    assert_eq!(m.launch.kind, qrtz_modules::manifest::LaunchKind::Package);
    assert_eq!(ext.entry, m.launch.entry);
    assert_eq!(ext.channels, m.launch.channels);
    assert_eq!(ext.views, m.views);
    assert_eq!(ext.capabilities, m.capabilities);
    assert_eq!(ext.permissions, m.permissions);
    assert_eq!(ext.settings, m.settings);
    assert_eq!(ext.min_app, m.min_app);
    assert!(
        q.dependencies.is_empty() && m.dependencies.is_empty(),
        "snap needs no model or service"
    );
    assert!(ext.reads.is_empty());
    assert_eq!(m.code.bundle_id, "ai.qrtz.snap");

    let lock = Lockfile::parse(bundled::LOCK.as_bytes()).unwrap();
    let l = &lock.modules["snap"];
    assert_eq!(l.manifest_sha256, sha256_hex(bundled::SNAP.as_bytes()));
    assert_eq!(l.version, m.version);
    assert!(l.dependencies.is_empty());
}

#[test]
fn snap_uses_accessibility_and_input_monitoring_and_carries_no_entitlement() {
    let m = ModuleManifest::parse(bundled::SNAP.as_bytes()).unwrap();
    let perms = serde_json::to_value(&m.permissions).unwrap();
    // Input monitoring: summon agent's ⌘+⌘ listen-only tap (asked only when summon turns on).
    assert_eq!(perms, serde_json::json!(["accessibility", "input-monitoring"]));
    assert!(m.entitlements.is_empty());
    assert_eq!(m.settings.namespace, "snap");
    assert_eq!(m.settings.legacy_sections, vec!["snap".to_string()]);
}
