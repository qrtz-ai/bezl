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

/// Design call (c) fixtures: what "codesign" and "notarytool" say about the code, without a Mac.
struct Fixture {
    cdhash: String,
    log: Option<String>,
}
impl qrtz_publish::notary::CodeEvidence for Fixture {
    fn cdhash(&self, _code: &Path) -> Result<String, String> {
        Ok(self.cdhash.clone())
    }
    fn notary_log(&self, _id: &str) -> Result<Option<String>, String> {
        Ok(self.log.clone())
    }
}

const REAL_ID: &str = "2b5c1d0e-8f3a-4c6b-9d7e-1a2b3c4d5e6f";

/// A record as publish.sh writes it: the real submission id, the file's sha256, its cdhash.
fn real(dir: PathBuf, cdhash: &str) -> SpecPackage {
    let mut p = SpecPackage::fake_notarized(dir);
    let n = p.notarization.as_mut().unwrap();
    n.submission_id = REAL_ID.into();
    n.cdhash = cdhash.into();
    p
}

fn publish_with(base: &Path, p: SpecPackage, how: &qrtz_publish::notary::Notarized) -> Result<(), String> {
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
    qrtz_publish::build_with(&out, &spec, &keys, now, how).map(|_| ())
}

fn log(job: &str, status: &str, cdhash: &str) -> String {
    json!({"jobId": job, "status": status, "statusSummary": "Ready for distribution",
           "sha256": "c".repeat(64), "issues": null,
           "ticketContents": [{"path": "code.zip/qrtz-echo", "digestAlgorithm": "SHA-256",
                               "cdhash": cdhash, "arch": "arm64"}]})
    .to_string()
}

/// Design call (c): a record whose cdhash isn't the code's is never signed into the index (nothing is published:
/// no timestamp, no index), and neither is one Apple's log disagrees with.
#[test]
fn a_record_whose_cdhash_isnt_the_codes_is_refused_before_signing() {
    use qrtz_publish::notary::Notarized;
    let on_disk = "1".repeat(40);

    let d = tempfile::tempdir().unwrap();
    let ev = Fixture {
        cdhash: "2".repeat(40),
        log: None,
    };
    let e = publish_with(d.path(), real(ext(d.path()), &on_disk), &Notarized::Checked(&ev)).unwrap_err();
    assert!(e.contains("echo@0.3.0: notarization:"), "{e}");
    assert!(e.contains(&format!("cdhash {} isn't", "2".repeat(40))), "{e}");
    assert!(
        qrtz_publish::current(&d.path().join("out")).unwrap().is_none(),
        "nothing signed"
    );
    assert!(!d.path().join("out/v1/timestamp.json").exists());

    // The code changed after notarization (sha256 differs): refused before its cdhash is even read.
    let d = tempfile::tempdir().unwrap();
    let dir = ext(d.path());
    let p = real(dir.clone(), &on_disk);
    write(&dir.join("bin/qrtz-echo"), b"echo helper, rebuilt", true);
    let ev = Fixture {
        cdhash: on_disk.clone(),
        log: None,
    };
    let e = publish_with(d.path(), p, &Notarized::Checked(&ev)).unwrap_err();
    assert!(
        e.contains("sha256") && e.contains("changed after notarization"),
        "{e}"
    );

    // The same cdhash: published. With Apple's log: it must be this submission, Accepted, ticketed for it.
    for (lg, ok) in [
        (None, None),
        (Some(log(REAL_ID, "Accepted", &on_disk)), None),
        (
            Some(log(&REAL_ID.to_uppercase(), "Accepted", &on_disk.to_uppercase())),
            None,
        ),
        (Some(log(REAL_ID, "Invalid", &on_disk)), Some("not Accepted")),
        (
            Some(log(REAL_ID, "Accepted", &"3".repeat(40))),
            Some("has no cdhash"),
        ),
        (
            Some(log("9b5c1d0e-8f3a-4c6b-9d7e-1a2b3c4d5e6f", "Accepted", &on_disk)),
            Some("not 2b5c1d0e"),
        ),
        (Some("<html>".into()), Some("isn't JSON")),
    ] {
        let d = tempfile::tempdir().unwrap();
        let ev = Fixture {
            cdhash: on_disk.to_uppercase(),
            log: lg.clone(),
        };
        let r = publish_with(d.path(), real(ext(d.path()), &on_disk), &Notarized::Checked(&ev));
        match ok {
            None => r.unwrap_or_else(|e| panic!("{lg:?}: {e}")),
            Some(why) => {
                let e = r.unwrap_err();
                assert!(e.contains(why), "{lg:?}: {e}");
                assert!(qrtz_publish::current(&d.path().join("out")).unwrap().is_none());
            }
        }
    }
}

/// A dev index's FAKE records go in only with `--dev-index`, which takes nothing else; a checked build refuses them.
#[test]
fn fake_records_need_dev_index_and_dev_index_takes_only_fake_records() {
    use qrtz_publish::notary::Notarized;
    let ev = Fixture {
        cdhash: "1".repeat(40),
        log: None,
    };
    let d = tempfile::tempdir().unwrap();
    let e = publish_with(
        d.path(),
        SpecPackage::fake_notarized(ext(d.path())),
        &Notarized::Checked(&ev),
    )
    .unwrap_err();
    assert!(e.contains("FAKE"), "{e}");
    let d = tempfile::tempdir().unwrap();
    publish_with(
        d.path(),
        SpecPackage::fake_notarized(ext(d.path())),
        &Notarized::DevIndex,
    )
    .unwrap();
    let d = tempfile::tempdir().unwrap();
    let e = publish_with(
        d.path(),
        real(ext(d.path()), &"1".repeat(40)),
        &Notarized::DevIndex,
    )
    .unwrap_err();
    assert!(e.contains("--dev-index takes only FAKE records"), "{e}");
}

/// The command: off macOS a plain `build` can't read a cdhash, so it publishes no code at all; `--dev-index` still
/// builds a dev index from FAKE records.
#[cfg(not(target_os = "macos"))]
#[test]
fn the_build_command_checks_records_and_refuses_code_it_cant_read() {
    use std::process::Command;
    let bin = || Command::new(env!("CARGO_BIN_EXE_qrtz-publish"));
    let d = tempfile::tempdir().unwrap();
    let keys = d.path().join("keys");
    for r in ["root-a", "timestamp", "index"] {
        assert!(bin()
            .args(["keygen", "--role", r, "--dir"])
            .arg(&keys)
            .output()
            .unwrap()
            .status
            .success());
    }
    let out = d.path().join("out");
    assert!(bin()
        .args(["root", "--out"])
        .arg(&out)
        .arg("--keys")
        .arg(&keys)
        .output()
        .unwrap()
        .status
        .success());
    let spec = |p: SpecPackage| {
        let n = serde_json::to_value(p.notarization.unwrap()).unwrap();
        let f = d.path().join("publish.json");
        std::fs::write(
            &f,
            serde_json::to_vec(&json!({"teamId": TEAM, "packages": [{"dir": p.dir, "notarization": n}]}))
                .unwrap(),
        )
        .unwrap();
        f
    };
    let build = |f: &Path, dev: bool| {
        let mut c = bin();
        c.args(["build", "--out"])
            .arg(&out)
            .arg("--spec")
            .arg(f)
            .arg("--keys")
            .arg(&keys);
        if dev {
            c.arg("--dev-index");
        }
        c.env_remove("QRTZ_NOTARY_PROFILE");
        c.output().unwrap()
    };
    let o = build(&spec(real(ext(d.path()), &"1".repeat(40))), false);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success() && err.contains("needs macOS"), "{err}");
    assert!(qrtz_publish::current(&out).unwrap().is_none());
    let o = build(&spec(SpecPackage::fake_notarized(ext(d.path()))), false);
    assert!(String::from_utf8_lossy(&o.stderr).contains("FAKE"));
    let o = build(&spec(SpecPackage::fake_notarized(ext(d.path()))), true);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(qrtz_publish::current(&out).unwrap().is_some());
}
