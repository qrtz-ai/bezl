//! ADR 0146 §6: per-module settings files, and global settings that only control all of them.
mod common;
use qrtz_modules::settings::{GlobalSettings, ModuleSettingsStore};
use serde_json::json;

#[test]
fn missing_switches_default_to_on_so_m0_changes_nothing() {
    let g = GlobalSettings::from_config(&json!({}));
    assert!(["echo", "nano", "ctrl"].iter().all(|m| g.is_on(m)));
    let g = GlobalSettings::from_config(&json!({"modules": {"nano": {"enabled": false}}}));
    assert!(g.is_on("echo") && !g.is_on("nano"));
    let g = GlobalSettings::from_config(&json!({"modules": {"enabled": false, "echo": {"enabled": true}}}));
    assert!(!g.is_on("echo"), "the all-modules switch wins");
    assert!(!g.is_on("other"));
}

#[test]
fn each_module_has_its_own_file_and_cant_write_global_keys() {
    let dir = tempfile::tempdir().unwrap();
    let s = ModuleSettingsStore::new(dir.path());
    assert_eq!(s.get("echo").unwrap(), json!({}));
    s.set("echo", &json!({"pushKey": "fn", "pill": {"show": true}}))
        .unwrap();
    s.set("echo", &json!({"pill": {"size": 2}})).unwrap();
    assert_eq!(
        s.get("echo").unwrap(),
        json!({"pushKey": "fn", "pill": {"show": true, "size": 2}})
    );
    assert_eq!(s.get("nano").unwrap(), json!({}), "echo's settings stay echo's");
    assert!(dir.path().join("modules/echo/settings.json").exists());
    for g in ["theme", "logLevel", "modules", "onboarding"] {
        assert!(s.set("echo", &json!({g: 1})).is_err(), "{g}");
    }
    // ADR 0146 M2.3: privacy is nano's now; the speech downloads stay global even inside `speech`.
    assert!(s
        .set("echo", &json!({"speech": {"warmAfterDownload": false}}))
        .is_err());
    assert!(s.set("other", &json!({})).is_err());
    assert!(s.set("../echo", &json!({})).is_err());
    s.set("echo", &json!({"__proto__": {"x": 1}})).unwrap();
    assert!(s.get("echo").unwrap().get("__proto__").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(dir.path().join("modules/echo/settings.json")).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o600);
    }
    // A corrupt file reads as defaults and isn't rewritten by a read.
    std::fs::create_dir_all(dir.path().join("modules/nano")).unwrap();
    std::fs::write(dir.path().join("modules/nano/settings.json"), b"{oops").unwrap();
    assert_eq!(s.get("nano").unwrap(), json!({}));
}

#[test]
fn a_module_sees_its_settings_and_a_read_only_copy_of_the_global_ones() {
    let dir = tempfile::tempdir().unwrap();
    let s = ModuleSettingsStore::new(dir.path());
    s.set("ctrl", &json!({"cadence": "hourly"})).unwrap();
    let cfg =
        json!({"theme": "light", "logLevel": "debug", "modules": {"ctrl": {"enabled": false}}, "talk": {}});
    assert_eq!(
        s.effective("ctrl", &cfg).unwrap(),
        json!({"global": {"theme": "light", "logLevel": "debug", "enabled": false}, "module": {"cadence": "hourly"}})
    );
}
