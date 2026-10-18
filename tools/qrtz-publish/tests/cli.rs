//! The publisher CLI: dry-run end to end (including a chunked file), keygen, key-file permissions, immutability,
//! and that no key material reaches stdout or the output dir.
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_qrtz-publish"))
}

fn rm(p: &std::path::Path) {
    let _ = qrtz_modules::fsutil::remove_tree(p);
}

#[test]
fn dry_run_publishes_packs_and_installs_with_runtime_keys() {
    let d = tempfile::tempdir().unwrap();
    let o = bin()
        .args(["dry-run", "--dir"])
        .arg(d.path())
        .arg("--big")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{out}{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("client sideload installed echo@0.3.0"), "{out}");
    assert!(out.contains("blobs checked"), "{out}");
    // The big file went out as two chunks, and no file in the output is larger than one.
    let blobs = d.path().join("out/v1/blobs/sha256");
    let sizes: Vec<u64> = std::fs::read_dir(&blobs)
        .unwrap()
        .flatten()
        .map(|e| e.metadata().unwrap().len())
        .collect();
    assert!(sizes.iter().all(|s| *s <= qrtz_modules::index::CHUNK_SIZE));
    assert!(
        sizes.contains(&qrtz_modules::index::CHUNK_SIZE) && sizes.contains(&4096),
        "{sizes:?}"
    );
    // No .key files anywhere: dry-run keys live in memory only.
    let mut stack = vec![d.path().to_path_buf()];
    while let Some(p) = stack.pop() {
        for e in std::fs::read_dir(&p).unwrap().flatten() {
            let q = e.path();
            if q.is_dir() {
                stack.push(q);
            } else {
                assert!(!q.to_string_lossy().ends_with(".key"), "{}", q.display());
            }
        }
    }
    rm(d.path());
}

#[test]
fn keygen_writes_0600_and_never_prints_the_secret() {
    let d = tempfile::tempdir().unwrap();
    let o = bin()
        .args(["keygen", "--role", "index", "--dir"])
        .arg(d.path())
        .output()
        .unwrap();
    assert!(o.status.success());
    let secret = std::fs::read_to_string(d.path().join("index.key")).unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!all.contains(secret.trim()), "the secret was printed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(d.path().join("index.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        // A readable key is refused.
        std::fs::set_permissions(d.path().join("index.key"), std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = qrtz_publish::keys::Keys::from_dir(d.path()).unwrap_err();
        assert!(e.contains("0600") && !e.contains(secret.trim()), "{e}");
    }
    // Never overwrites.
    let o = bin()
        .args(["keygen", "--role", "index", "--dir"])
        .arg(d.path())
        .output()
        .unwrap();
    assert!(!o.status.success());
}

#[test]
fn a_published_version_never_changes() {
    let d = tempfile::tempdir().unwrap();
    let keys = qrtz_publish::keys::Keys::ephemeral();
    let out = d.path().join("out");
    let now = qrtz_publish::now_ms();
    qrtz_publish::write_root(&out, &keys, None, 1, now).unwrap();
    let spec = qrtz_publish::dry_run::sample(d.path(), false).unwrap();
    qrtz_publish::build(&out, &spec, &keys, now).unwrap();
    // Same content again: nothing new, versions still move.
    let b = qrtz_publish::build(&out, &spec, &keys, now + 1).unwrap();
    assert!(b.added.is_empty());
    assert_eq!((b.index_version, b.timestamp_version), (2, 2));
    // Other bytes under a published version: refused.
    std::fs::write(spec.packages[0].dir.join("config.json"), b"{\"changed\":true}").unwrap();
    let e = qrtz_publish::build(&out, &spec, &keys, now + 2).unwrap_err();
    assert!(e.contains("bump the version"), "{e}");
    // The timestamp re-sign keeps the index.
    let t = qrtz_publish::resign_timestamp(&out, &keys, now + 3).unwrap();
    assert_eq!((t.version, t.index.version), (3, 2));
    rm(d.path());
}

#[test]
fn keys_can_come_from_the_environment() {
    let d = tempfile::tempdir().unwrap();
    for r in ["root-a", "timestamp", "index"] {
        assert!(bin()
            .args(["keygen", "--role", r, "--dir"])
            .arg(d.path())
            .output()
            .unwrap()
            .status
            .success());
    }
    let read = |n: &str| std::fs::read_to_string(d.path().join(n)).unwrap();
    let out = d.path().join("out");
    let o = bin()
        .args(["root", "--out"])
        .arg(&out)
        .env("QRTZ_PUBLISH_ROOT_KEYS", read("root-a.key"))
        .env("QRTZ_PUBLISH_TIMESTAMP_KEY", read("timestamp.key"))
        .env("QRTZ_PUBLISH_INDEX_KEY", read("index.key"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.starts_with("root 1: sha256 "), "{s}");
    for n in ["root-a.key", "timestamp.key", "index.key"] {
        assert!(!s.contains(read(n).trim()));
    }
    rm(d.path());
}
