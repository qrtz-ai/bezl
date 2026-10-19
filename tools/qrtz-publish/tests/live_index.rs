//! A (package, version) already in the live index is immutable: a build whose entry differs is refused before
//! anything is signed (publish.sh passes the host's current index as --live-index).
use qrtz_publish::keys::Keys;
use qrtz_publish::notary::Notarized;
use qrtz_publish::{Spec, SpecPackage};
use serde_json::json;
use std::path::{Path, PathBuf};

const TEAM: &str = "ABCDE12345";
const NOW: u64 = 1_800_000_000_000;

fn ext(base: &Path, tag: &str, version: &str, code: &[u8]) -> PathBuf {
    let d = base.join(format!("src/{tag}"));
    std::fs::create_dir_all(d.join("bin")).unwrap();
    std::fs::write(
        d.join("qrtz.json"),
        serde_json::to_vec(&json!({"name": "snap", "version": version, "kind": "extension",
            "extension": {"entry": "bin/qrtz-snap", "protocol": 1, "minApp": "0.2.0"}}))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(d.join("bin/qrtz-snap"), code).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d.join("bin/qrtz-snap"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    d
}

fn spec(dir: PathBuf) -> Spec {
    Spec {
        team_id: TEAM.into(),
        packages: vec![SpecPackage::fake_notarized(dir)],
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    }
}

/// A fresh output dir (as on a Mac whose local out is missing or stale) built against `live`.
fn build_fresh(base: &Path, name: &str, dir: PathBuf, live: &qrtz_modules::index::Index) -> Result<(), String> {
    let keys = Keys::ephemeral();
    let out = base.join(name);
    qrtz_publish::write_root(&out, &keys, None, 1, NOW).unwrap();
    qrtz_publish::build_against(&out, &spec(dir), &keys, NOW, &Notarized::Unchecked, Some(live))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[test]
fn a_live_version_with_other_content_is_refused_and_a_bump_passes() {
    let d = tempfile::tempdir().unwrap();
    let base = d.path();
    let keys = Keys::ephemeral();
    let live_out = base.join("live");
    qrtz_publish::write_root(&live_out, &keys, None, 1, NOW).unwrap();
    qrtz_publish::build(&live_out, &spec(ext(base, "a", "0.1.0", b"snap one")), &keys, NOW).unwrap();
    let (_, live) = qrtz_publish::current(&live_out).unwrap().unwrap();

    let e = build_fresh(base, "o1", ext(base, "b", "0.1.0", b"snap two"), &live).unwrap_err();
    assert!(e.contains("snap@0.1.0") && e.contains("live index"), "{e}");
    assert!(!base.join("o1/v1/timestamp.json").exists(), "nothing signed");

    build_fresh(base, "o2", ext(base, "c", "0.1.0", b"snap one"), &live).expect("same content republishes");
    build_fresh(base, "o3", ext(base, "d", "0.1.1", b"snap two"), &live).expect("a new version passes");
}
