//! ADR 0153 §3: the production root 1 that `crates/qrtz-app/src/module_hosts.json` pins. Its public copy and
//! detached signatures live in `crates/qrtz-modules/roots/` (the same bytes as https://modules.qrtz.ai/v1/root/1.root.json).
//! It must hash to the pin, parse, and carry threshold 2 on the root role, met by both root keys; one root
//! signature alone must not be enough.
use qrtz_modules::index::{IndexError, Root, Signatures};
use qrtz_modules::trust::Trust;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pin() -> String {
    let hosts: serde_json::Value =
        serde_json::from_slice(&std::fs::read(repo().join("crates/qrtz-app/src/module_hosts.json")).unwrap())
            .unwrap();
    hosts["root1Sha256"]
        .as_str()
        .expect("module_hosts.json pins root1Sha256")
        .to_owned()
}

fn files() -> (Vec<u8>, Vec<u8>) {
    let d = repo().join("crates/qrtz-modules/roots");
    (
        std::fs::read(d.join("1.root.json")).unwrap(),
        std::fs::read(d.join("1.root.json.sig")).unwrap(),
    )
}

#[test]
fn the_checked_in_root_is_the_pinned_one() {
    let (root, _) = files();
    assert_eq!(qrtz_modules::sha256_hex(&root), pin());
}

#[test]
fn the_pinned_root_bootstraps_with_both_root_signatures() {
    let (bytes, sig) = files();
    let trust = Trust::bootstrap(Some(&pin()), &bytes, &sig).expect("root 1 verifies against the pin");
    let root = &trust.root;
    assert_eq!(root.version, 1);
    assert_eq!(root.roles.root.threshold, 2, "root 1 needs both root keys");
    assert_eq!(root.roles.root.keyids.len(), 2);
    assert_eq!(root.roles.index.threshold, 1);
    assert_eq!(root.roles.timestamp.threshold, 1);
    // Four distinct keys: root-a, root-b, index, timestamp.
    assert_eq!(root.keys.len(), 4);
    let sigs = Signatures::parse(&sig).unwrap();
    for id in &root.roles.root.keyids {
        assert!(
            sigs.signatures.iter().any(|s| &s.keyid == id),
            "root key {id} signed root 1"
        );
    }
}

#[test]
fn one_root_signature_is_not_enough() {
    let (bytes, sig) = files();
    let root = Root::parse(&bytes).unwrap();
    let all = Signatures::parse(&sig).unwrap();
    for keep in &all.signatures {
        let one = Signatures {
            signatures: vec![keep.clone()],
        };
        let e = root
            .verify_role("root", &root.roles.root, &bytes, &one)
            .unwrap_err();
        assert!(matches!(e, IndexError::Signature(_)), "{e}");
    }
    // Each signature on its own is a good signature by its key (threshold-1 view of the same role).
    for keep in &all.signatures {
        let mut role = root.roles.root.clone();
        role.keyids = vec![keep.keyid.clone()];
        role.threshold = 1;
        let one = Signatures {
            signatures: vec![keep.clone()],
        };
        root.verify_role("root", &role, &bytes, &one)
            .expect("a valid signature");
    }
}

#[test]
fn a_changed_byte_breaks_the_pin() {
    let (mut bytes, sig) = files();
    let last = bytes.len() - 2;
    bytes[last] ^= 1;
    assert!(Trust::bootstrap(Some(&pin()), &bytes, &sig).is_err());
}
