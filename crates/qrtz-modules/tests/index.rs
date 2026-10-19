//! ADR 0153/0157: the signed index format and the trust checks, without a publisher or a network.
use ed25519_dalek::{Signer, SigningKey};
use qrtz_modules::index::*;
use qrtz_modules::trust::{Mode, Trust};
use std::collections::BTreeMap;

const NOW: u64 = 1_800_000_000_000;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn id(k: &SigningKey) -> String {
    key_id(&k.verifying_key().to_bytes())
}

fn sign(b: &[u8], ks: &[&SigningKey]) -> Vec<u8> {
    let mut signatures: Vec<Signature> = ks
        .iter()
        .map(|k| Signature {
            keyid: id(k),
            sig: hex::encode(k.sign(b).to_bytes()),
        })
        .collect();
    signatures.sort_by(|a, b| a.keyid.cmp(&b.keyid));
    canonical(&Signatures { signatures })
}

fn root(version: u64, roots: &[&SigningKey], threshold: u32, ts: &SigningKey, ix: &SigningKey) -> Vec<u8> {
    let mut keys = BTreeMap::new();
    for k in roots.iter().chain([&ts, &ix]) {
        keys.insert(
            id(k),
            PublicKey {
                scheme: "ed25519".into(),
                public: hex::encode(k.verifying_key().to_bytes()),
            },
        );
    }
    let mut rids: Vec<String> = roots.iter().map(|k| id(k)).collect();
    rids.sort();
    canonical(&Root {
        kind: "root".into(),
        version,
        issued_ms: NOW - 1000,
        expires_ms: NOW + 300 * DAY_MS,
        keys,
        roles: Roles {
            root: Role {
                keyids: rids,
                threshold,
            },
            timestamp: Role {
                keyids: vec![id(ts)],
                threshold: 1,
            },
            index: Role {
                keyids: vec![id(ix)],
                threshold: 1,
            },
        },
    })
}

fn entry() -> PackageEntry {
    let m = br#"{"kind":"model","model":{"task":"asr"},"name":"model.x","version":"1.0.0"}"#;
    let tree = Tree::new(vec![
        IndexFile {
            path: "a.bin".into(),
            size: 3,
            sha256: qrtz_modules::sha256_hex(b"abc"),
            executable: false,
            mirrors: vec![
                "https://huggingface.co/o/r/resolve/0123abc000000000000000000000000000000000/a.bin".into(),
            ],
        },
        IndexFile {
            path: "qrtz.json".into(),
            size: m.len() as u64,
            sha256: qrtz_modules::sha256_hex(m),
            executable: false,
            mirrors: vec![],
        },
    ]);
    PackageEntry {
        kind: PackageKind::Model,
        team_id: "ABCDE12345".into(),
        sha256: tree.sha256(),
        size: tree.size(),
        manifest_sha256: qrtz_modules::sha256_hex(m),
        files: tree.files,
        dependencies: BTreeMap::new(),
        identifier: None,
        designated_requirement: None,
        task: Some("asr".into()),
        models: vec![],
        platform: PLATFORM.into(),
        min_os: None,
        notarization: None,
    }
}

fn index(version: u64) -> Vec<u8> {
    canonical(&Index {
        kind: "index".into(),
        version,
        issued_ms: NOW - 1000,
        expires_ms: NOW + 300 * DAY_MS,
        revoked: vec![],
        packages: BTreeMap::from([(
            "model.x".to_string(),
            BTreeMap::from([("1.0.0".to_string(), entry())]),
        )]),
    })
}

fn stamp(version: u64, ix: &[u8], ix_version: u64, issued: u64, days: u64) -> Vec<u8> {
    canonical(&Timestamp {
        kind: "timestamp".into(),
        version,
        issued_ms: issued,
        expires_ms: issued + days * DAY_MS,
        index: IndexRef {
            sha256: qrtz_modules::sha256_hex(ix),
            size: ix.len() as u64,
            version: ix_version,
        },
    })
}

struct Keys {
    r: SigningKey,
    t: SigningKey,
    i: SigningKey,
}

fn setup() -> (Keys, Trust) {
    let k = Keys {
        r: key(1),
        t: key(2),
        i: key(3),
    };
    let r1 = root(1, &[&k.r], 1, &k.t, &k.i);
    let pin = qrtz_modules::sha256_hex(&r1);
    let t = Trust::bootstrap(Some(&pin), &r1, &sign(&r1, &[&k.r])).unwrap();
    (k, t)
}

#[test]
fn layout_paths_are_the_four_kinds_and_nothing_else() {
    let h = "a".repeat(64);
    assert_eq!(layout::root(3), "v1/root/3.root.json");
    assert_eq!(layout::timestamp(), "v1/timestamp.json");
    assert_eq!(layout::index(&h), format!("v1/index/{h}.json"));
    assert_eq!(layout::blob(&h), format!("v1/blobs/sha256/{h}"));
    for ok in [
        layout::root(1),
        layout::sig(&layout::root(12)),
        layout::timestamp(),
        layout::index(&h),
        layout::blob(&h),
    ] {
        assert!(layout::is_valid(&ok), "{ok}");
    }
    for bad in [
        "v1/root/0.root.json",
        "v1/root/01.root.json",
        "v2/timestamp.json",
        "v1/blobs/sha256/../x",
        "v1/index/abc.json",
        &format!("{}.sig", layout::blob(&h)),
        "v1/timestamp.json?x=1",
    ] {
        assert!(!layout::is_valid(bad), "{bad}");
    }
}

#[test]
fn a_package_is_its_tree_and_the_tree_hash_is_its_sha256() {
    let e = entry();
    assert_eq!(e.sha256, qrtz_modules::sha256_hex(&e.tree().bytes()));
    // Its blobs: the tree itself, then each distinct file, with the file's mirrors.
    let b = e.blobs();
    assert_eq!(b[0].sha256, e.sha256);
    assert_eq!(b.len(), 3);
    assert!(b.iter().any(|x| x.mirrors
        == vec![
            "https://huggingface.co/o/r/resolve/0123abc000000000000000000000000000000000/a.bin".to_string()
        ]));
    for bad in [
        "http://huggingface.co/x",
        "https://user@huggingface.co/x",
        "https://huggingface.co/x?download=1",
        "https://huggingface.co/../x",
        "https://huggingface.co",
        "file:///etc/passwd",
    ] {
        assert!(!is_mirror_url(bad), "{bad}");
    }
}

#[test]
fn documents_must_be_canonical() {
    let ix = index(1);
    Index::parse(&ix).unwrap();
    let pretty =
        serde_json::to_vec_pretty(&serde_json::from_slice::<serde_json::Value>(&ix).unwrap()).unwrap();
    assert_eq!(Index::parse(&pretty), Err(IndexError::NotCanonical("index")));
    let float = br#"{"expiresMs":1.5,"issuedMs":1,"packages":{},"type":"index","version":1}"#;
    assert!(Index::parse(float).is_err());
}

#[test]
fn index_entries_are_checked() {
    let mut v: serde_json::Value = serde_json::from_slice(&index(1)).unwrap();
    let e = &mut v["packages"]["model.x"]["1.0.0"];
    e["sha256"] = serde_json::json!("0".repeat(64));
    assert!(Index::parse(&serde_json::to_vec(&v).unwrap())
        .unwrap_err()
        .to_string()
        .contains("sha256 of the tree"));
    let mut v: serde_json::Value = serde_json::from_slice(&index(1)).unwrap();
    v["packages"]["model.x"]["1.0.0"]["files"][0]["path"] = serde_json::json!("../a.bin");
    assert!(Index::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    for (field, value) in [
        ("kind", serde_json::json!("extension")),
        ("identifier", serde_json::json!("ai.qrtz.other")),
        ("dependencies", serde_json::json!({"model.y": "^1.0.0"})),
        ("task", serde_json::Value::Null),
    ] {
        let mut v: serde_json::Value = serde_json::from_slice(&index(1)).unwrap();
        if value.is_null() {
            v["packages"]["model.x"]["1.0.0"]
                .as_object_mut()
                .unwrap()
                .remove(field);
        } else {
            v["packages"]["model.x"]["1.0.0"][field] = value;
        }
        assert!(Index::parse(&serde_json::to_vec(&v).unwrap()).is_err(), "{field}");
    }
    // A name that isn't a package name, or whose kind isn't what it says.
    let mut v: serde_json::Value = serde_json::from_slice(&index(1)).unwrap();
    let e = v["packages"]["model.x"].take();
    v["packages"].as_object_mut().unwrap().remove("model.x");
    v["packages"]["evil"] = e;
    assert!(Index::parse(&serde_json::to_vec(&v).unwrap()).is_err());
}

#[test]
fn a_timestamp_lives_at_most_8_days() {
    let ix = index(1);
    Timestamp::parse(&stamp(1, &ix, 1, NOW, 8)).unwrap();
    assert!(Timestamp::parse(&stamp(1, &ix, 1, NOW, 9)).is_err());
}

#[test]
fn bootstrap_needs_the_pin_and_the_roots_own_signature() {
    let (k, _) = setup();
    let r1 = root(1, &[&k.r], 1, &k.t, &k.i);
    assert_eq!(
        Trust::bootstrap(None, &r1, &sign(&r1, &[&k.r])),
        Err(IndexError::NoPinnedRoot)
    );
    assert_eq!(
        Trust::bootstrap(Some(&"0".repeat(64)), &r1, &sign(&r1, &[&k.r])),
        Err(IndexError::UnpinnedRoot)
    );
    let pin = qrtz_modules::sha256_hex(&r1);
    assert_eq!(
        Trust::bootstrap(Some(&pin), &r1, &sign(&r1, &[&k.t])),
        Err(IndexError::Signature("root"))
    );
}

#[test]
fn a_pair_verifies_and_freshness_applies_only_to_the_network_path() {
    let (k, t) = setup();
    let ix = index(1);
    let ts = stamp(1, &ix, 1, NOW, 7);
    let v = t
        .verify_pair(
            Mode::Network,
            &ts,
            &sign(&ts, &[&k.t]),
            &ix,
            &sign(&ix, &[&k.i]),
            NOW,
        )
        .unwrap();
    assert_eq!(v.snapshot().version, 1);
    // Signed by the wrong role.
    assert_eq!(
        t.verify_pair(
            Mode::Network,
            &ts,
            &sign(&ts, &[&k.i]),
            &ix,
            &sign(&ix, &[&k.i]),
            NOW
        ),
        Err(IndexError::Signature("timestamp"))
    );
    assert_eq!(
        t.verify_pair(
            Mode::Network,
            &ts,
            &sign(&ts, &[&k.t]),
            &ix,
            &sign(&ix, &[&k.t]),
            NOW
        ),
        Err(IndexError::Signature("index"))
    );
    // The index must be the one the timestamp names.
    let other = index(2);
    assert_eq!(
        t.verify_pair(
            Mode::Network,
            &ts,
            &sign(&ts, &[&k.t]),
            &other,
            &sign(&other, &[&k.i]),
            NOW
        ),
        Err(IndexError::TimestampMismatch("index"))
    );
    // Expired: refused on the network path, allowed (and reported later) on a sideload.
    let late = NOW + 8 * DAY_MS;
    assert_eq!(
        t.verify_pair(
            Mode::Network,
            &ts,
            &sign(&ts, &[&k.t]),
            &ix,
            &sign(&ix, &[&k.i]),
            late
        ),
        Err(IndexError::Expired { what: "timestamp" })
    );
    t.verify_pair(
        Mode::Sideload,
        &ts,
        &sign(&ts, &[&k.t]),
        &ix,
        &sign(&ix, &[&k.i]),
        late,
    )
    .unwrap();
}

#[test]
fn versions_only_go_up_and_never_change_content() {
    let (k, mut t) = setup();
    let ix1 = index(1);
    let ts2 = stamp(2, &ix1, 1, NOW, 7);
    let v = t
        .verify_pair(
            Mode::Network,
            &ts2,
            &sign(&ts2, &[&k.t]),
            &ix1,
            &sign(&ix1, &[&k.i]),
            NOW,
        )
        .unwrap();
    t.commit(&v, NOW).unwrap();
    let ts1 = stamp(1, &ix1, 1, NOW, 7);
    assert!(matches!(
        t.verify_pair(
            Mode::Network,
            &ts1,
            &sign(&ts1, &[&k.t]),
            &ix1,
            &sign(&ix1, &[&k.i]),
            NOW
        ),
        Err(IndexError::Rollback {
            what: "timestamp",
            got: 1,
            stored: 2
        })
    ));
    // Same version, other bytes.
    let ts2b = stamp(2, &ix1, 1, NOW + 1, 7);
    assert!(matches!(
        t.verify_pair(
            Mode::Network,
            &ts2b,
            &sign(&ts2b, &[&k.t]),
            &ix1,
            &sign(&ix1, &[&k.i]),
            NOW
        ),
        Err(IndexError::Equivocation { .. })
    ));
    // The clock high-water mark: a clock wound back still sees the expiry.
    let ts3 = stamp(3, &ix1, 1, NOW + 6 * DAY_MS, 1);
    let v = t
        .verify_pair(
            Mode::Network,
            &ts3,
            &sign(&ts3, &[&k.t]),
            &ix1,
            &sign(&ix1, &[&k.i]),
            NOW + 6 * DAY_MS,
        )
        .unwrap();
    t.commit(&v, NOW + 6 * DAY_MS).unwrap();
    assert!(t.effective_now(NOW) >= NOW + 6 * DAY_MS);
}

#[test]
fn a_root_rotation_needs_both_thresholds() {
    let (k, mut t) = setup();
    let (a, b) = (key(10), key(11));
    let r2 = root(2, &[&a, &b], 2, &k.t, &k.i);
    assert_eq!(
        t.update_root(&r2, &sign(&r2, &[&a, &b])),
        Err(IndexError::Signature("root (old keys)"))
    );
    assert_eq!(
        t.update_root(&r2, &sign(&r2, &[&k.r, &a])),
        Err(IndexError::Signature("root (new keys)"))
    );
    let r3 = root(3, &[&a, &b], 2, &k.t, &k.i);
    assert!(
        t.update_root(&r3, &sign(&r3, &[&k.r, &a, &b])).is_err(),
        "no skipping a version"
    );
    t.update_root(&r2, &sign(&r2, &[&k.r, &a, &b])).unwrap();
    assert_eq!(t.root.version, 2);
}

#[test]
fn trust_state_round_trips() {
    let (_, t) = setup();
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("trust.json");
    t.save(&p).unwrap();
    assert_eq!(Trust::load(&p).unwrap().unwrap(), t);
}

/// ADR 0171 (902bcab review): a model's weight files carry exactly their Hugging Face URL at a pinned commit, and
/// nothing else in a package carries a URL. Each mutation below must fail on the URL rule itself (the tree is
/// recomputed, so the tree hash can't be what refuses it), and the unmutated entry must pass, so the check can't
/// pass vacuously.
#[test]
fn a_model_file_has_only_its_pinned_hugging_face_url() {
    const GOOD: &str = "https://huggingface.co/o/r/resolve/0123abc000000000000000000000000000000000/a.bin";
    let with = |weights: Vec<&str>, manifest: Vec<&str>| -> Result<(), String> {
        let mut e = entry();
        e.files[0].mirrors = weights.into_iter().map(str::to_owned).collect();
        e.files[1].mirrors = manifest.into_iter().map(str::to_owned).collect();
        let t = Tree::new(e.files.clone());
        e.sha256 = t.sha256();
        e.size = t.size();
        e.files = t.files;
        e.validate("model.x")
    };
    assert_eq!(with(vec![GOOD], vec![]), Ok(()));
    let c40 = "0123abc000000000000000000000000000000000";
    for (bad, why) in [
        (vec![], "needs its pinned Hugging Face URL"),
        (
            vec!["https://huggingface.co/o/r/resolve/main/a.bin"],
            "pinned commit",
        ),
        (
            vec!["https://huggingface.co/o/r/resolve/0123abc/a.bin"],
            "pinned commit",
        ),
        (
            vec![&*format!("https://huggingface.co/o/r/blob/{c40}/a.bin")],
            "pinned commit",
        ),
        (
            vec![&*format!("https://hf.example/o/r/resolve/{c40}/a.bin")],
            "pinned commit",
        ),
        (
            vec![&*format!("https://huggingface.co.evil/o/r/resolve/{c40}/a.bin")],
            "pinned commit",
        ),
        (
            vec![GOOD, "https://modules.qrtz.ai/v1/blobs/sha256/00"],
            "pinned commit",
        ),
        (
            vec![&*format!("http://huggingface.co/o/r/resolve/{c40}/a.bin")],
            "https URL",
        ),
    ] {
        let err = with(bad.clone(), vec![]).unwrap_err();
        assert!(err.contains(why), "{bad:?}: {err}");
    }
    let err = with(vec![GOOD], vec![GOOD]).unwrap_err();
    assert!(err.contains("only a model's files have URLs"), "{err}");
}
