#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0215: a published version is pulled only by listing it in the repo's revocation list (modules/revoked.json,
//! passed as the spec's `revoked`). The live index 3933c81e… (index 2, spec/fixtures/ctrl-live-index) carries nano
//! 0.2.0 and 0.2.1, which declare no reads for their three models (ADR 0212) and so can never pass the read check;
//! a build on top of it passes only with both revoked, the entries stay (immutable), and no client resolves them.
use qrtz_modules::index::{layout, Index};
use qrtz_modules::qrtz_lock::{QrtzLock, Workspace};
use qrtz_modules::resolve::{resolve, ResolveError, Update};
use qrtz_modules::sideload::Revoked;
use qrtz_publish::keys::Keys;
use qrtz_publish::notary::Notarized;
use qrtz_publish::Spec;
use std::path::{Path, PathBuf};

const NOW: u64 = 1_800_000_000_000;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/fixtures/ctrl-live-index")
}

fn live() -> Index {
    Index::parse(&std::fs::read(fixture().join("index.json")).unwrap()).unwrap()
}

/// An output dir whose current index is the live index 2 (as on the release Mac).
fn out_at_live(base: &Path, keys: &Keys) -> PathBuf {
    let out = base.join("out");
    qrtz_publish::write_root(&out, keys, None, 1, NOW).unwrap();
    let ix = std::fs::read(fixture().join("index.json")).unwrap();
    let p = out.join(layout::index(&qrtz_modules::sha256_hex(&ix)));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, ix).unwrap();
    std::fs::write(
        out.join(layout::timestamp()),
        std::fs::read(fixture().join("timestamp.json")).unwrap(),
    )
    .unwrap();
    out
}

fn repo_revocations() -> Vec<Revoked> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules/revoked.json");
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

fn spec(revoked: Vec<Revoked>) -> Spec {
    Spec {
        team_id: "ABCDE12345".into(),
        packages: vec![],
        pinned_models: vec![],
        models: vec![],
        revoked,
    }
}

fn build(revoked: Vec<Revoked>) -> Result<qrtz_publish::Built, String> {
    let d = tempfile::tempdir().unwrap();
    let keys = Keys::ephemeral();
    let out = out_at_live(d.path(), &keys);
    let lv = live();
    qrtz_publish::build_against(&out, &spec(revoked), &keys, NOW, &Notarized::Unchecked, Some(&lv)).inspect(
        |_| {
            let (_, ix) = qrtz_publish::current(&out).unwrap().unwrap();
            // Nothing is dropped: every live entry is still there, byte-for-byte, and nano 0.2.0/0.2.1 are revoked.
            assert_eq!(ix.packages, lv.packages);
            assert!(ix.is_revoked("nano", "0.2.0") && ix.is_revoked("nano", "0.2.1"));
        },
    )
}

#[test]
fn the_live_index_2_builds_only_with_the_repo_revocations_of_nano_0_2_0_and_0_2_1() {
    let e = build(vec![]).unwrap_err();
    assert!(
        e.contains("nano@0.2.") && e.contains("nothing it runs can read"),
        "{e}"
    );
    let r = repo_revocations();
    assert!(r
        .iter()
        .any(|x| x.package == "nano" && x.versions == ["0.2.0", "0.2.1"]));
    let b = build(r).unwrap();
    assert_eq!(b.revoked.len(), 1, "{:?}", b.revoked);
    assert!(b.revoked[0].starts_with("nano@0.2.0,0.2.1 ("), "{:?}", b.revoked);
    // Revoking only one of them is not enough: every resolvable entry must pass.
    let one = vec![Revoked {
        package: "nano".into(),
        versions: vec!["0.2.0".into()],
        reason: "test".into(),
    }];
    let e = build(one).unwrap_err();
    assert!(e.contains("nano@0.2.1"), "{e}");
}

#[test]
fn a_revoked_version_is_never_resolved() {
    let mut ix = live();
    ix.revoked = repo_revocations();
    let mut ws = Workspace::default();
    ws.dependencies.insert("nano".into(), "^0.2.0".into());
    let e = resolve(&ix, &ws, &QrtzLock::default(), &Update::None).unwrap_err();
    assert!(
        matches!(e, ResolveError::NotFound { ref name, .. } if name == "nano"),
        "{e}"
    );
    let mut ws = Workspace::default();
    ws.dependencies.insert("nano".into(), "=0.2.1".into());
    assert!(resolve(&ix, &ws, &QrtzLock::default(), &Update::None).is_err());
}

#[test]
fn a_revocation_needs_a_reason() {
    for (versions, reason) in [(vec![], ""), (vec!["0.2.0".to_string()], " ")] {
        let e = build(vec![Revoked {
            package: "nano".into(),
            versions,
            reason: reason.into(),
        }])
        .unwrap_err();
        assert!(e.contains("ADR 0215"), "{e}");
    }
}

/// There is no removal path: an index is always the previous one plus what the spec adds, so a version left out of
/// the spec (and not in modules/revoked.json) stays resolvable, and a live version with other content is refused.
#[test]
fn an_unlisted_version_is_never_dropped_or_replaced() {
    let d = tempfile::tempdir().unwrap();
    let keys = Keys::ephemeral();
    let out = out_at_live(d.path(), &keys);
    let lv = live();
    qrtz_publish::build_against(
        &out,
        &spec(repo_revocations()),
        &keys,
        NOW,
        &Notarized::Unchecked,
        Some(&lv),
    )
    .unwrap();
    let (_, ix) = qrtz_publish::current(&out).unwrap().unwrap();
    for (name, vs) in &lv.packages {
        for v in vs.keys() {
            assert!(ix.entry(name, v).is_some(), "{name}@{v} dropped");
            if name != "nano" {
                assert!(!ix.is_revoked(name, v), "{name}@{v} revoked without being listed");
            }
        }
    }
}

/// `publish.sh --release --only nano`: the spec carries only nano 0.2.2. ctrl's and snap's repo manifests (bumped by
/// 2fe6dee) are never built, so every other package keeps its live entries byte-for-byte and no new ctrl or snap
/// version appears; the full read check runs over every resolvable entry and the index is version 3.
#[test]
fn only_nano_leaves_every_other_package_identical_to_the_live_index() {
    let d = tempfile::tempdir().unwrap();
    let keys = Keys::ephemeral();
    let out = out_at_live(d.path(), &keys);
    let lv = live();
    let src = d.path().join("nano");
    std::fs::create_dir_all(src.join("bin")).unwrap();
    std::fs::write(
        src.join("qrtz.json"),
        serde_json::to_vec(
            &serde_json::json!({"name": "nano", "version": "0.2.2", "kind": "extension",
            "extension": {"entry": "bin/qrtz-nano", "protocol": 1, "minApp": "0.2.0"}}),
        )
        .unwrap(),
    )
    .unwrap();
    std::fs::write(src.join("bin/qrtz-nano"), b"nano 0.2.2").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(src.join("bin/qrtz-nano"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut s = spec(repo_revocations());
    s.packages = vec![qrtz_publish::SpecPackage::fake_notarized(src)];
    let b = qrtz_publish::build_against(&out, &s, &keys, NOW, &Notarized::Unchecked, Some(&lv)).unwrap();
    assert_eq!(b.index_version, 3);
    assert_eq!(b.added, vec!["nano@0.2.2".to_string()]);
    let (_, ix) = qrtz_publish::current(&out).unwrap().unwrap();
    for (name, vs) in &lv.packages {
        if name == "nano" {
            for (v, e) in vs {
                assert_eq!(ix.entry(name, v), Some(e));
            }
        } else {
            assert_eq!(ix.packages.get(name), Some(vs), "{name} changed");
        }
    }
    assert_eq!(ix.packages.len(), lv.packages.len());
    assert!(
        ix.is_revoked("nano", "0.2.0") && ix.is_revoked("nano", "0.2.1") && !ix.is_revoked("nano", "0.2.2")
    );
    assert_eq!(ix.latest("nano").map(|(v, _)| v), Some("0.2.2"));
}
