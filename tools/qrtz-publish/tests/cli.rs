//! The publisher CLI: dry-run end to end (extension, service and model, a file big enough for range fetches),
//! keygen, key-file permissions, immutability, pinned models, and that no key material reaches stdout or the output.
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
    assert!(out.contains("client sideload installed 3 packages"), "{out}");
    assert!(
        out.contains("inference.sample@1.0.0, model.sample@1.0.0, nano@0.3.0"),
        "{out}"
    );
    assert!(out.contains("blobs checked"), "{out}");
    // The big file is one whole blob (fetched in ranges, never split into objects). ADR 0171: a model file is kept
    // beside the output dir, never in it (modules.qrtz.ai doesn't host model bytes; the pack still carries it).
    let sizes = |p: &str| -> Vec<u64> {
        std::fs::read_dir(d.path().join(p))
            .unwrap()
            .flatten()
            .map(|e| e.metadata().unwrap().len())
            .collect()
    };
    assert!(sizes("out.model-blobs/v1/blobs/sha256").contains(&(20 * 1024 * 1024)));
    assert!(!sizes("out/v1/blobs/sha256").contains(&(20 * 1024 * 1024)));
    // The client's lock is text and names all three, exactly.
    let lock = std::fs::read_to_string(d.path().join("client/qrtz.lock")).unwrap();
    for k in [
        "\"nano@0.3.0\"",
        "\"inference.sample@1.0.0\"",
        "\"model.sample@1.0.0\"",
        "\"designatedRequirement\"",
    ] {
        assert!(lock.contains(k), "{k} in {lock}");
    }
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
    let cfg = spec.packages[0].dir.join("config.json");
    std::fs::write(&cfg, b"{\"changed\":true}").unwrap();
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

#[test]
fn a_pinned_model_publishes_as_exactly_the_tree_the_app_pins() {
    let d = tempfile::tempdir().unwrap();
    let keys = qrtz_publish::keys::Keys::ephemeral();
    let out = d.path().join("out");
    let now = qrtz_publish::now_ms();
    qrtz_publish::write_root(&out, &keys, None, 1, now).unwrap();
    // A pin file like local_models.json, and the downloaded files.
    let files = d.path().join("dl");
    std::fs::create_dir_all(files.join("m.mlmodelc")).unwrap();
    std::fs::write(files.join("m.mlmodelc/weight.bin"), b"weights").unwrap();
    std::fs::write(files.join("vocab.json"), b"{}").unwrap();
    let f = |p: &str, b: &[u8]| serde_json::json!({"remote": format!("up/{p}"), "path": p, "size": b.len(), "sha256": qrtz_modules::sha256_hex(b)});
    let pins = serde_json::json!({"version": "1.0.0", "base": "https://huggingface.co", "sets": [{
        "id": "x", "package": "model.x", "task": "asr", "dir": "x", "repo": "org/x", "revision": "abc1230000000000000000000000000000000000",
        "files": [f("m.mlmodelc/weight.bin", b"weights"), f("vocab.json", b"{}")]}]});
    std::fs::write(d.path().join("pins.json"), serde_json::to_vec(&pins).unwrap()).unwrap();
    let spec = serde_json::json!({"teamId": "ABCDE12345", "pinnedModels": ["pins.json"],
                                  "models": [{"name": "model.x", "dir": "dl"}]});
    std::fs::write(d.path().join("publish.json"), serde_json::to_vec(&spec).unwrap()).unwrap();
    let spec = qrtz_publish::Spec::load(&d.path().join("publish.json")).unwrap();
    qrtz_publish::build(&out, &spec, &keys, now).unwrap();
    let (_, ix) = qrtz_publish::current(&out).unwrap().unwrap();
    let e = ix.entry("model.x", "1.0.0").unwrap();
    let src = &qrtz_modules::models::parse_pinned(&serde_json::to_vec(&pins).unwrap()).unwrap()[0];
    assert_eq!(
        e.sha256,
        src.tree().sha256(),
        "the hosted tree is the pinned tree"
    );
    assert!(e.files.iter().any(|f| f.mirrors
        == vec![
            "https://huggingface.co/org/x/resolve/abc1230000000000000000000000000000000000/up/vocab.json"
                .to_string()
        ]));
    // A downloaded file that doesn't match its pin is refused.
    std::fs::write(files.join("vocab.json"), b"{ }").unwrap();
    let e = qrtz_publish::build(&out, &spec, &keys, now + 1).unwrap_err();
    assert!(e.contains("doesn't match the pin"), "{e}");
    rm(d.path());
}

#[test]
fn upload_list_puts_the_timestamp_pair_last_and_no_model_bytes() {
    // ADR 0171: what an upload without rclone (the Cloudflare connector) needs, in order.
    let d = tempfile::tempdir().unwrap();
    let o = bin().args(["dry-run", "--dir"]).arg(d.path()).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = d.path().join("out");
    let list = qrtz_publish::upload_list(&out, false).unwrap();
    let keys: Vec<&str> = list.iter().map(|u| u.key.as_str()).collect();
    let n = keys.len();
    assert_eq!(
        keys[n - 2..],
        ["v1/timestamp.json.sig", "v1/timestamp.json"],
        "{keys:?}"
    );
    assert!(list[n - 1].cache_control.contains("max-age=300"));
    assert!(list[..n - 2]
        .iter()
        .all(|u| u.cache_control.contains("immutable")));
    assert!(keys.iter().all(|k| k.starts_with("v1/")), "{keys:?}");
    assert!(keys.iter().any(|k| k.starts_with("v1/index/")), "{keys:?}");
    // Every listed object is a file of out, and every file of out's v1/ is listed.
    for u in &list {
        assert_eq!(
            std::fs::metadata(out.join(&u.key)).unwrap().len(),
            u.size,
            "{}",
            u.key
        );
    }
    let model = std::fs::read_dir(d.path().join("out.model-blobs/v1/blobs/sha256")).unwrap();
    for e in model.flatten() {
        let k = format!("v1/blobs/sha256/{}", e.file_name().to_string_lossy());
        assert!(!keys.contains(&k.as_str()), "model bytes listed: {k}");
    }
    let ts = qrtz_publish::upload_list(&out, true).unwrap();
    let ts: Vec<&str> = ts.iter().map(|u| u.key.as_str()).collect();
    assert_eq!(ts, ["v1/timestamp.json.sig", "v1/timestamp.json"]);
    // The CLI prints the same, tab-separated.
    let o = bin().args(["upload-list", "--out"]).arg(&out).output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    assert_eq!(s.lines().count(), n);
    assert!(s.lines().last().unwrap().starts_with("v1/timestamp.json\t"));
}

#[test]
fn model_files_lists_the_pinned_hugging_face_urls() {
    let pins = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/qrtz-app/src/local_models.json"
    );
    let o = bin()
        .args(["model-files", "--pins", pins, "--package", "model.nemotron"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.lines().count() > 3, "{s}");
    for l in s.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        assert_eq!(f.len(), 4, "{l}");
        assert!(qrtz_modules::index::is_pinned_hugging_face_url(f[0]), "{l}");
        assert_eq!(f[3].len(), 64, "{l}");
    }
}
