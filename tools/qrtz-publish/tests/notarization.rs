//! ADR 0170: code is published only notarized, and the record is checked.
use qrtz_modules::index::Notarization;
use qrtz_publish::keys::Keys;
use qrtz_publish::{Spec, SpecPackage};
use serde_json::json;
use std::path::{Path, PathBuf};

const TEAM: &str = "ABCDE12345";

fn write(p: &Path, b: &[u8], exec: bool) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
    #[cfg(unix)]
    if exec {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn ext(base: &Path) -> PathBuf {
    let d = base.join("src/echo");
    write(
        &d.join("qrtz.json"),
        &serde_json::to_vec(&json!({"name": "echo", "version": "0.3.0", "kind": "extension",
            "extension": {"entry": "bin/qrtz-echo", "protocol": 1, "minApp": "0.3.0"}}))
        .unwrap(),
        false,
    );
    write(&d.join("bin/qrtz-echo"), b"echo helper", true);
    write(&d.join("README"), b"not code", false);
    d
}

fn publish(base: &Path, p: SpecPackage) -> Result<(), String> {
    let keys = Keys::ephemeral();
    let out = base.join("out");
    let now = 1_800_000_000_000;
    qrtz_publish::write_root(&out, &keys, None, 1, now).unwrap();
    let spec = Spec {
        team_id: TEAM.into(),
        packages: vec![p],
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    };
    qrtz_publish::build(&out, &spec, &keys, now)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn good() -> Notarization {
    Notarization {
        submission_id: "2b5c1d0e-8f3a-4c6b-9d7e-1a2b3c4d5e6f".into(),
        status: "Accepted".into(),
        code_sha256: "a".repeat(64),
        cdhash: "b".repeat(40),
    }
}

#[test]
fn a_record_is_well_formed_and_accepted_or_refused() {
    assert_eq!(good().check(), Ok(()));
    for (bad, why) in [
        (
            Notarization {
                status: "Invalid".into(),
                ..good()
            },
            "Accepted",
        ),
        (
            Notarization {
                submission_id: "not-a-uuid".into(),
                ..good()
            },
            "UUID",
        ),
        (
            Notarization {
                code_sha256: "a".repeat(63),
                ..good()
            },
            "sha256",
        ),
        (
            Notarization {
                cdhash: "B".repeat(40),
                ..good()
            },
            "cdhash",
        ),
    ] {
        let e = bad.check().unwrap_err();
        assert!(e.contains(why), "{e}");
    }
    // Unknown fields in the signed index are refused, not ignored.
    let mut v = serde_json::to_value(good()).unwrap();
    v["ticket"] = json!("x");
    assert!(serde_json::from_value::<Notarization>(v).is_err());
}

#[test]
fn code_without_an_accepted_record_never_enters_the_index() {
    let d = tempfile::tempdir().unwrap();
    let e = publish(d.path(), SpecPackage::new(ext(d.path()))).unwrap_err();
    assert!(e.contains("notarized"), "{e}");

    let d = tempfile::tempdir().unwrap();
    let mut p = SpecPackage::fake_notarized(ext(d.path()));
    p.notarization.as_mut().unwrap().status = "In Progress".into();
    assert!(publish(d.path(), p).is_err());

    // A record for a file that isn't the package's code (here: a non-executable file of it) is refused.
    let d = tempfile::tempdir().unwrap();
    let dir = ext(d.path());
    let readme = qrtz_modules::sha256_hex(b"not code");
    let mut p = SpecPackage::fake_notarized(dir);
    p.notarization.as_mut().unwrap().code_sha256 = readme;
    assert!(publish(d.path(), p).is_err());

    let d = tempfile::tempdir().unwrap();
    publish(d.path(), SpecPackage::fake_notarized(ext(d.path()))).unwrap();
}
