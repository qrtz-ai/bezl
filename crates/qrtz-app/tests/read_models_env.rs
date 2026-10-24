//! ADR 0212, at launch: a downloaded module's helper gets `QRTZ_READ_MODELS` with exactly its granted read-only
//! store dirs (empty when it has none, so it never falls back to a writable view). ADR 0237: echo, nano and snap
//! are bundled and get none; ctrl is bundled but keeps its laya grant (`BUNDLED_PACKAGE_READS`), so it still gets
//! the var.
use qrtz_app::installed::read_models_env;

#[test]
fn only_a_module_with_package_reads_gets_the_var() {
    let d = tempfile::tempdir().unwrap();
    for m in ["echo", "nano", "snap"] {
        assert_eq!(read_models_env(d.path(), m), None, "{m}");
    }
    // Nothing installed: no grants, but the var is set (empty), so ctrl never falls back to <data>/models.
    assert_eq!(read_models_env(d.path(), "ctrl").as_deref(), Some(""));
}
