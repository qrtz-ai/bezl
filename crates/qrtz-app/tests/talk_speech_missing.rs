#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! N182 (ADR 0177): talk's "echo's speech model isn't downloaded" probe. A fresh qrtz has echo on before Parakeet is
//! on the Mac (ADR 0175); when the helper then refuses a press for want of a recogniser, the pill must point at the
//! download, not "try again in a moment". The probe answers from echo's own sets only (marker + sizes).
use qrtz_app::local_models::{LocalModels, Manifest, MARKER};
use qrtz_app::talk::echo_speech_missing;
use serde_json::json;

fn manifest(dirs: &[&str]) -> Manifest {
    let sets: Vec<_> = dirs
        .iter()
        .map(|dir| {
            json!({"id": dir, "package": format!("model.{dir}"), "task": "asr", "dir": dir,
            "repo": format!("org/{dir}"), "revision": "c".repeat(40),
            "files": [{"remote": "w.bin", "path": "w.bin", "size": 1024, "sha256": "0".repeat(64)}]})
        })
        .collect();
    serde_json::from_value(json!({"version": "1.0.0", "base": "https://huggingface.co", "sets": sets}))
        .unwrap()
}

fn install(root: &std::path::Path, dir: &str) {
    let d = root.join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::File::create(d.join("w.bin"))
        .unwrap()
        .set_len(1024)
        .unwrap();
    std::fs::write(d.join(MARKER), "1.0.0").unwrap();
}

#[test]
fn missing_until_echos_own_model_is_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("models");
    let l = LocalModels::with_manifest(&root, manifest(&["parakeet-tdt-v2", "nemotron-560ms"]), None);
    let missing = echo_speech_missing(Some(&l));
    assert!(missing(), "fresh: echo's model isn't there");
    // Nano's model alone doesn't make echo ready.
    install(&root, "nemotron-560ms");
    assert!(missing(), "nano's model is not echo's");
    // A half-written set (no marker yet) is still missing.
    let d = root.join("parakeet-tdt-v2");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::File::create(d.join("w.bin"))
        .unwrap()
        .set_len(1024)
        .unwrap();
    assert!(missing(), "no marker: still downloading");
    install(&root, "parakeet-tdt-v2");
    assert!(!missing(), "downloaded: the probe follows the disk, no restart");
    // Deleted from settings: missing again.
    std::fs::remove_dir_all(&d).unwrap();
    assert!(missing());
}

#[test]
fn no_local_models_or_no_echo_set_is_never_missing() {
    assert!(
        !echo_speech_missing(None)(),
        "a build without local models keeps the old message"
    );
    let tmp = tempfile::tempdir().unwrap();
    let l = LocalModels::with_manifest(tmp.path(), manifest(&["nemotron-560ms"]), None);
    assert!(
        !echo_speech_missing(Some(&l))(),
        "echo needs no set here: nothing to download"
    );
}
