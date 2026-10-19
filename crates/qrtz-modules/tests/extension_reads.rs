//! ADR 0157 §9 (Bozniak): ctrl reading `model.laya` in its own process is the one, time-boxed `extension.reads`
//! exception (exit: a sandboxed `inference.laya` XPC service). Manifest validation refuses any other
//! `extension.reads`; the cap at one exception is offline_graph's `at_most_one_extension_reads_a_model_in_its_own_process`.
use qrtz_modules::qrtz_json::PackageManifest;

fn ext(name: &str, deps: serde_json::Value, reads: serde_json::Value) -> Result<PackageManifest, String> {
    let v = serde_json::json!({"name": name, "version": "1.0.0", "kind": "extension", "dependencies": deps,
        "extension": {"entry": "bin/h", "protocol": 1, "minApp": "0.3.0", "reads": reads}});
    PackageManifest::parse(&serde_json::to_vec(&v).unwrap()).map_err(|e| e.to_string())
}

#[test]
fn only_ctrl_may_read_only_model_laya() {
    ext(
        "ctrl",
        serde_json::json!({"model.laya": "^1.0.0"}),
        serde_json::json!(["model.laya"]),
    )
    .unwrap();
    // Another extension reading laya, ctrl reading another model, or a second read: refused.
    for (name, deps, reads) in [
        (
            "echo",
            serde_json::json!({"model.laya": "^1.0.0"}),
            serde_json::json!(["model.laya"]),
        ),
        (
            "ctrl",
            serde_json::json!({"model.nemotron": "^1.0.0"}),
            serde_json::json!(["model.nemotron"]),
        ),
        (
            "ctrl",
            serde_json::json!({"model.laya": "^1.0.0", "model.nemotron": "^1.0.0"}),
            serde_json::json!(["model.laya", "model.nemotron"]),
        ),
    ] {
        let e = ext(name, deps, reads).unwrap_err();
        assert!(e.contains("extension.reads"), "{name}: {e}");
    }
    // ctrl must depend on what it reads.
    assert!(ext("ctrl", serde_json::json!({}), serde_json::json!(["model.laya"])).is_err());
}
