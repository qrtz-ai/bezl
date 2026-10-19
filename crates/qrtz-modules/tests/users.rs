//! `QrtzLock::users`: the one "who uses this" answer the module manager, `qrtz ls` and prune's dry run share.
use qrtz_modules::qrtz_lock::QrtzLock;
use serde_json::json;

fn locked(kind: &str, deps: serde_json::Value) -> serde_json::Value {
    let code = kind != "model";
    let mut l = json!({"kind": kind, "sha256": "a".repeat(64), "size": 1, "teamId": "MR26CCZ496",
                       "dependencies": deps, "source": "network"});
    if code {
        l["identifier"] = json!("x");
        l["designatedRequirement"] = json!("x");
    }
    l
}

#[test]
fn users_walk_up_to_the_extensions_and_name_what_the_workspace_pinned() {
    let lock: QrtzLock = serde_json::from_value(json!({
        "lockfileVersion": 1,
        "workspace": {
            "dependencies": {"echo": "^0.3.0", "model.nemotron": "=1.0.0", "inference.voice": "^1.0.0",
                             "model.loose": "=1.0.0"},
            "resolved": {"echo": "0.3.0", "model.nemotron": "1.0.0", "inference.voice": "1.0.0",
                         "model.loose": "1.0.0"}
        },
        "packages": {
            "echo@0.3.0": locked("extension", json!({"inference.voice": "1.0.0", "model.parakeet": "1.0.0"})),
            "inference.voice@1.0.0": locked("service", json!({"model.voice": "1.0.0"})),
            "model.voice@1.0.0": locked("model", json!({})),
            "model.parakeet@1.0.0": locked("model", json!({})),
            "model.nemotron@1.0.0": locked("model", json!({})),
            "model.loose@1.0.0": locked("model", json!({})),
        }
    }))
    .unwrap();
    let pinned = |n: &str| match n {
        "model.nemotron" => vec!["nano".to_owned()],
        _ => vec![],
    };
    // Through a service: the extension above it, not the service.
    assert_eq!(lock.users("model.voice", "1.0.0", &pinned), ["echo"]);
    assert_eq!(lock.users("model.parakeet", "1.0.0", &pinned), ["echo"]);
    // A workspace pin: the module it is for (the case Jared saw as "used by nothing").
    assert_eq!(lock.users("model.nemotron", "1.0.0", &pinned), ["nano"]);
    // A service the user asked for by name, and a bare request nothing is known to use.
    assert_eq!(lock.users("inference.voice", "1.0.0", &pinned), ["echo"]);
    assert!(lock.users("model.loose", "1.0.0", &pinned).is_empty());
    assert!(lock.users("model.nemotron", "1.0.0", &|_| vec![]).is_empty());
}
