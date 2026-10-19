//! ADR 0212, at launch: a downloaded module's helper gets `QRTZ_READ_MODELS` with exactly its granted read-only
//! store dirs (empty when it has none, so it never falls back to a writable view); bundled echo gets none.
use qrtz_app::installed::read_models_env;

#[test]
fn a_downloaded_module_always_gets_the_var_and_bundled_echo_never_does() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(read_models_env(d.path(), "echo"), None);
    // Not installed: no grants, but the var is set (empty), so nothing falls back to <data>/models.
    assert_eq!(read_models_env(d.path(), "nano").as_deref(), Some(""));
    assert_eq!(read_models_env(d.path(), "ctrl").as_deref(), Some(""));
}
