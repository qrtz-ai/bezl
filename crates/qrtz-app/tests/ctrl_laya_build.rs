//! QRT-332: the app's macOS dependency on qrtz-ctrl enables `laya`, so release builds can run the decision model.
//! The compile-time assert in lib.rs enforces it on macOS; this keeps Linux CI from losing the wiring silently.

#[test]
fn macos_app_builds_enable_laya() {
    let manifest = include_str!("../Cargo.toml");
    let mac = manifest
        .split("[target.'cfg(target_os = \"macos\")'.dependencies]")
        .nth(1)
        .expect("macOS target dependencies section");
    let mac = mac.split("\n[").next().unwrap_or_default();
    assert!(
        mac.lines()
            .any(|l| l.starts_with("qrtz-ctrl") && l.contains("\"laya\"")),
        "qrtz-ctrl with features = [\"laya\"] under the macOS target dependencies"
    );
}

#[test]
fn runtime_build_matches_target() {
    assert_eq!(qrtz_ctrl::laya::RUNTIME_BUILD, cfg!(target_os = "macos"));
}
