//! ADR 0144 §6 gate (g), on ADR 0157's installer: Bozniak's sideload freshness cases driven through
//! `qrtz_modules::install::Installer` (`sideload`, `refresh`, `plan`/`apply`) on the real publisher's signed output,
//! not the legacy `qrtz_modules::sideload::decide` (its unit cases stay in `crates/qrtz-modules/tests/gate_g.rs`).
//!
//! - g1: a `.qrtzpkg` sideload with an expired revocation list installs after the age prompt, marked
//!   `staleRevocation` (every package in the file's set).
//! - g2: a sideload the (carried or stored) list revokes is refused outright, before the confirm prompt.
//! - g3: a file whose carried snapshot is older than the stored one is refused (rollback).
//! - g4: a later fresh index that revokes the sideloaded extension disables it.
//! - Freshness exemption (ADR 0157 §6, 0144 §7): a data-only model that no extension reads in its own process may
//!   install without a fresh list; code never may, nor `model.laya` (ctrl reads it in-process, `extension.reads`).
//!
//! `scripts/modules/mac-gate.sh` SKIPs (g) on the Mac: it is installer logic, and this is where it runs.
use qrtz_modules::fetch::{Source, Transport};
use qrtz_modules::index::{layout, PackageKind, DAY_MS};
use qrtz_modules::install::{InstallError, Installer, Op, RefreshConsent};
use qrtz_modules::models::{ModelFile, ModelSource};
use qrtz_modules::qrtz_lock::Source as LockSource;
use qrtz_modules::sideload::{Confirmation, Refusal, Revoked, SideloadOrigin};
use qrtz_publish::dry_run::NoAppleChecks;
use qrtz_publish::keys::Keys;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const TEAM: &str = "ABCDE12345";

/// Hosts are publisher output dirs; mirror URL prefixes map to dirs.
struct Dirs {
    hosts: Vec<PathBuf>,
    urls: BTreeMap<String, PathBuf>,
}

#[async_trait::async_trait]
impl Transport for Dirs {
    fn hosts(&self) -> usize {
        self.hosts.len()
    }
    async fn get(&self, host: usize, path: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(self.hosts[host].join(path)) {
            Ok(b) if b.len() > max => Err("too large".into()),
            Ok(b) => Ok(Some(b)),
            Err(_) => Ok(None),
        }
    }
    async fn get_range(&self, src: &Source, sha: &str, start: u64, len: u64) -> Result<Vec<u8>, String> {
        let p = match src {
            Source::Host(i) => self.hosts[*i].join(layout::blob(sha)),
            Source::Url(u) => {
                let (pre, dir) = self
                    .urls
                    .iter()
                    .find(|(p, _)| u.starts_with(*p))
                    .ok_or("unknown mirror")?;
                dir.join(&u[pre.len()..])
            }
        };
        let b = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let end = (start + len) as usize;
        if end > b.len() {
            return Err("range outside".into());
        }
        Ok(b[start as usize..end].to_vec())
    }
}

fn write(p: &Path, b: &[u8], exec: bool) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
    #[cfg(unix)]
    if exec {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

struct World {
    _d: tempfile::TempDir,
    base: PathBuf,
    out: PathBuf,
    keys: Keys,
    pin: String,
    now: u64,
    n: usize,
}

impl World {
    fn new() -> Self {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_path_buf();
        let keys = Keys::ephemeral();
        let out = base.join("out");
        let now = 1_800_000_000_000;
        let (_, pin) = qrtz_publish::write_root(&out, &keys, None, 1, now).unwrap();
        Self {
            _d: d,
            base,
            out,
            keys,
            pin,
            now,
            n: 0,
        }
    }
    fn dir(&mut self) -> PathBuf {
        self.n += 1;
        self.base.join(format!("src/{}", self.n))
    }
    fn json(&mut self, m: serde_json::Value, files: &[(&str, &[u8], bool)]) -> PathBuf {
        let d = self.dir();
        write(&d.join("qrtz.json"), &serde_json::to_vec(&m).unwrap(), false);
        for (p, b, x) in files {
            write(&d.join(p), b, *x);
        }
        d
    }
    fn model(&mut self, name: &str, task: &str, bytes: &[u8]) -> PathBuf {
        self.json(
            json!({"name": name, "version": "1.0.0", "kind": "model", "model": {"task": task}}),
            &[("weights.bin", bytes, false)],
        )
    }
    fn service(&mut self, name: &str, models: &[&str]) -> PathBuf {
        let code = format!("svc {name}");
        self.json(
            json!({"name": name, "version": "1.0.0", "kind": "service",
                "service": {"bundle": "s.xpc", "models": models}}),
            &[("s.xpc/Contents/MacOS/s", code.as_bytes(), true)],
        )
    }
    fn ext(&mut self, name: &str, deps: serde_json::Value, reads: &[&str]) -> PathBuf {
        let code = format!("{name} helper");
        let mut e = json!({"entry": "bin/helper", "protocol": 1, "minApp": "0.3.0"});
        if !reads.is_empty() {
            e["reads"] = json!(reads);
        }
        self.json(
            json!({"name": name, "version": "0.3.0", "kind": "extension", "dependencies": deps, "extension": e}),
            &[("bin/helper", code.as_bytes(), true)],
        )
    }
    fn publish(&self, dirs: Vec<PathBuf>, revoked: Vec<Revoked>, at: u64) {
        let spec = qrtz_publish::Spec {
            team_id: TEAM.into(),
            packages: dirs
                .into_iter()
                .map(|dir| qrtz_publish::SpecPackage { dir, min_os: None })
                .collect(),
            pinned_models: vec![],
            models: vec![],
            revoked,
        };
        qrtz_publish::build(&self.out, &spec, &self.keys, at).unwrap();
    }
    fn installer(&self, name: &str) -> Installer {
        Installer::new(
            self.base.join(name),
            Some(self.pin.clone()),
            Box::new(NoAppleChecks),
        )
    }
    fn hosts(&self) -> Dirs {
        Dirs {
            hosts: vec![self.out.clone()],
            urls: BTreeMap::new(),
        }
    }
    /// echo → inference.voice + parakeet.
    fn catalogue(&mut self) {
        let p = self.model("model.parakeet-tdt-v2-en", "asr", &[1u8; 4000]);
        let v = self.service("inference.voice", &["model.parakeet-tdt-v2-en"]);
        let e = self.ext(
            "echo",
            json!({"inference.voice": "^1.0.0", "model.parakeet-tdt-v2-en": "^1.0.0"}),
            &[],
        );
        self.publish(vec![p, v, e], vec![], self.now);
    }
    fn pack(&self, name: &str, file: &str) -> PathBuf {
        let f = self.base.join(file);
        qrtz_publish::pack(&self.out, name, "0.3.0", &f).unwrap();
        f
    }
}

fn revoke(package: &str, reason: &str) -> Revoked {
    Revoked {
        package: package.into(),
        versions: vec!["0.3.0".into()],
        reason: reason.into(),
    }
}

/// Ask once (no confirmation), then say yes to exactly that prompt.
async fn confirm(i: &Installer, f: &Path, now: u64) -> Confirmation {
    match i.sideload(f, SideloadOrigin::UserPickedFile, None, now).await {
        Err(InstallError::Refused(Refusal::NeedsConfirmation(p))) => Confirmation {
            package_sha256: p.package_sha256,
            revocation_issued_ms: p.revocation_issued_ms,
        },
        other => panic!("expected the confirm prompt, got {other:?}"),
    }
}

/// A pinned model tree as the app builds it (`qrtz_modules::models`), its one file served by a "Hugging Face" dir.
fn pinned(w: &World, package: &str, task: &str, version: &str, bytes: &[u8]) -> (ModelSource, Dirs) {
    let src = ModelSource {
        package: package.into(),
        version: version.into(),
        task: task.into(),
        repo: format!("org/{package}"),
        revision: "ee09c569".into(),
        license: String::new(),
        files: vec![ModelFile {
            remote: "w.bin".into(),
            path: "w.bin".into(),
            size: bytes.len() as u64,
            sha256: qrtz_modules::sha256_hex(bytes),
        }],
    };
    let hf = w.base.join(format!("hf-{package}-{version}"));
    write(
        &hf.join(format!("org/{package}/resolve/ee09c569/w.bin")),
        bytes,
        false,
    );
    let mut t = w.hosts();
    t.urls.insert("https://huggingface.co/".into(), hf);
    (src, t)
}

async fn apply_pinned(i: &Installer, t: &Dirs, src: &ModelSource, now: u64) -> Result<(), InstallError> {
    let plan = i.plan(Op::Pinned(vec![src.pinned(TEAM)]))?;
    i.apply(t, &plan, &plan.consent(), now, &|_, _| {})
        .await
        .map(|_| ())
}

#[tokio::test]
async fn g1_an_expired_list_sideload_installs_after_the_age_prompt_and_is_marked_stale() {
    let mut w = World::new();
    w.catalogue();
    let f = w.pack("echo", "echo.qrtzpkg");
    let i = w.installer("c");
    let late = w.now + 30 * DAY_MS;
    let prompt = match i.sideload(&f, SideloadOrigin::UserPickedFile, None, late).await {
        Err(InstallError::Refused(Refusal::NeedsConfirmation(p))) => p,
        other => panic!("{other:?}"),
    };
    assert!(prompt.revocation_expired, "the prompt says the list is expired");
    assert_eq!(prompt.revocation_age_ms, Some(30 * DAY_MS));
    assert!(
        i.lock().unwrap().packages.is_empty(),
        "nothing installed before the yes"
    );
    let yes = Confirmation {
        package_sha256: prompt.package_sha256,
        revocation_issued_ms: prompt.revocation_issued_ms,
    };
    let a = i
        .sideload(&f, SideloadOrigin::UserPickedFile, Some(&yes), late)
        .await
        .unwrap();
    assert!(a.stale_revocation);
    let lock = i.lock().unwrap();
    assert_eq!(
        lock.packages.len(),
        3,
        "exactly the file's set: {:?}",
        lock.packages.keys()
    );
    for (k, l) in &lock.packages {
        assert!(l.stale_revocation, "{k}: staleRevocation");
        assert_eq!(l.source, LockSource::Sideload, "{k}");
    }
}

#[tokio::test]
async fn g2_a_revoked_sideload_is_refused_before_the_confirm_prompt() {
    let mut w = World::new();
    w.catalogue();
    // The index now revokes echo 0.3.0; a pack still carries what is signed (the client refuses it).
    w.publish(vec![], vec![revoke("echo", "pulled")], w.now + 1);
    let f = w.pack("echo", "echo.qrtzpkg");
    let i = w.installer("c");
    let late = w.now + 30 * DAY_MS;
    // No confirmation given: the refusal comes first, there is no prompt to say yes to.
    let e = i
        .sideload(&f, SideloadOrigin::UserPickedFile, None, late)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, InstallError::Refused(Refusal::Revoked { package, .. }) if package == "echo"),
        "{e:?}"
    );
    assert!(i.lock().unwrap().packages.is_empty());
    // The same with a stored list that revokes it (a fresh refresh) and an older, unrevoked file: no override.
    let mut w = World::new();
    w.catalogue();
    let old = w.pack("echo", "old.qrtzpkg");
    let i = w.installer("c");
    i.refresh(&w.hosts(), &RefreshConsent::user_asked(), w.now)
        .await
        .unwrap();
    let yes = confirm(&i, &old, w.now).await;
    w.publish(vec![], vec![revoke("echo", "pulled")], w.now + 1);
    i.refresh(&w.hosts(), &RefreshConsent::user_asked(), w.now + 2)
        .await
        .unwrap();
    let e = i
        .sideload(&old, SideloadOrigin::UserPickedFile, Some(&yes), w.now + 3)
        .await
        .unwrap_err();
    assert!(
        matches!(
            &e,
            InstallError::Refused(Refusal::Revoked { .. })
                | InstallError::Refused(Refusal::RevocationRollback { .. })
        ),
        "{e:?}"
    );
    assert!(i.lock().unwrap().get("echo", "0.3.0").is_none());
}

#[tokio::test]
async fn g3_a_file_carrying_an_older_snapshot_than_the_stored_one_is_refused() {
    let mut w = World::new();
    w.catalogue();
    let old = w.pack("echo", "old.qrtzpkg");
    // A newer index (and so a newer revocation list) reaches this Mac online.
    let n = w.model("model.nemotron", "asr", &[2u8; 5000]);
    w.publish(vec![n], vec![], w.now + DAY_MS);
    let i = w.installer("c");
    i.refresh(&w.hosts(), &RefreshConsent::user_asked(), w.now + DAY_MS)
        .await
        .unwrap();
    let e = i
        .sideload(&old, SideloadOrigin::UserPickedFile, None, w.now + 2 * DAY_MS)
        .await
        .unwrap_err();
    assert!(
        matches!(
            &e,
            InstallError::Refused(Refusal::RevocationRollback { carried, stored }) if carried < stored
        ),
        "{e:?}"
    );
    assert!(i.lock().unwrap().packages.is_empty());
}

#[tokio::test]
async fn g4_a_later_fresh_index_that_revokes_the_sideloaded_extension_disables_it() {
    let mut w = World::new();
    w.catalogue();
    let f = w.pack("echo", "echo.qrtzpkg");
    let i = w.installer("c");
    let late = w.now + 30 * DAY_MS;
    let yes = confirm(&i, &f, late).await;
    i.sideload(&f, SideloadOrigin::UserPickedFile, Some(&yes), late)
        .await
        .unwrap();
    assert!(i.lock().unwrap().get("echo", "0.3.0").unwrap().disabled.is_none());
    w.publish(vec![], vec![revoke("echo", "pulled")], late + 1);
    let r = i
        .refresh(&w.hosts(), &RefreshConsent::user_asked(), late + 2)
        .await
        .unwrap();
    assert!(!r.notices.is_empty(), "the user is told");
    let lock = i.lock().unwrap();
    let echo = lock.get("echo", "0.3.0").unwrap();
    assert!(echo.disabled.is_some(), "disabled after the fresh list");
    // What the fresh list doesn't revoke is no longer stale.
    assert!(
        !lock
            .get("model.parakeet-tdt-v2-en", "1.0.0")
            .unwrap()
            .stale_revocation
    );
}

#[tokio::test]
async fn freshness_a_data_only_model_no_extension_reads_installs_without_a_fresh_list() {
    let w = World::new();
    let i = w.installer("c");
    // No index, no list at all: a hash-pinned data model still installs (ADR 0157 §6).
    let (src, t) = pinned(&w, "model.parakeet-tdt-v2-en", "asr", "1.0.0", b"weights");
    apply_pinned(&i, &t, &src, w.now).await.unwrap();
    let lock = i.lock().unwrap();
    let l = lock.get("model.parakeet-tdt-v2-en", "1.0.0").unwrap();
    assert_eq!(l.source, LockSource::Pinned);
    assert_eq!(l.kind, PackageKind::Model);
}

#[tokio::test]
async fn freshness_code_never_installs_without_a_fresh_list() {
    let mut w = World::new();
    w.catalogue();
    let i = w.installer("c");
    let t = w.hosts();
    i.refresh(&t, &RefreshConsent::user_asked(), w.now).await.unwrap();
    // Online: the list expired between the plan and the yes.
    let plan = i.plan(Op::Install(vec![("echo".into(), None)])).unwrap();
    assert_eq!(
        i.apply(&t, &plan, &plan.consent(), w.now + 8 * DAY_MS, &|_, _| {})
            .await
            .unwrap_err(),
        InstallError::Refused(Refusal::RevocationListExpired)
    );
    // Offline: code can't take the pinned (no-list) path at all.
    let (mut src, _) = pinned(&w, "model.x", "asr", "1.0.0", b"code");
    src.package = "inference.voice".into();
    assert!(matches!(
        i.plan(Op::Pinned(vec![src.pinned(TEAM)])),
        Err(InstallError::PinnedCode(_)) | Err(InstallError::ManifestMismatch { .. })
    ));
    assert!(i.lock().unwrap().packages.is_empty());
}

#[tokio::test]
#[ignore = "pending Bobs's installer rule (ADR 0157 §6 / 0144 §7): the freshness exemption is data-only AND no \
            extension.reads consumer; Installer::apply exempts every data-only pinned plan today, model.laya included"]
async fn freshness_model_laya_read_in_ctrls_process_needs_a_fresh_list() {
    let w = World::new();
    let i = w.installer("c");
    // ctrl reads model.laya in its own process (extension.reads), so it is treated like code: no list, no install.
    let (src, t) = pinned(&w, "model.laya", "decision", "1.0.0", b"laya");
    assert_eq!(
        apply_pinned(&i, &t, &src, w.now).await.unwrap_err(),
        InstallError::Refused(Refusal::RevocationListExpired)
    );
    assert!(i.lock().unwrap().packages.is_empty());
}

#[tokio::test]
#[ignore = "pending Bobs: (1) an extension with extension.reads can't be published today (the index entry refuses \
            `models` on anything but a service: \"ctrl@0.3.0: models is for services\"), and (2) the installer rule \
            (ADR 0157 §6 / 0144 §7) that a model an extension reads in-process needs a fresh list isn't in \
            Installer::apply yet"]
async fn freshness_a_model_an_installed_extension_reads_needs_a_fresh_list() {
    let mut w = World::new();
    w.catalogue();
    // ctrl → model.laya, read in ctrl's own process.
    let l = w.model("model.laya", "decision", &[3u8; 3000]);
    let c = w.ext("ctrl", json!({"model.laya": "^1.0.0"}), &["model.laya"]);
    w.publish(vec![l, c], vec![], w.now);
    let i = w.installer("c");
    let t = w.hosts();
    i.refresh(&t, &RefreshConsent::user_asked(), w.now).await.unwrap();
    let plan = i.plan(Op::Install(vec![("ctrl".into(), None)])).unwrap();
    i.apply(&t, &plan, &plan.consent(), w.now, &|_, _| {})
        .await
        .unwrap();
    let late = w.now + 8 * DAY_MS;
    // The list has expired. Another data model nobody reads in-process still installs; a new laya doesn't.
    let (p, tp) = pinned(&w, "model.parakeet-tdt-v2-en", "asr", "1.0.0", b"weights");
    apply_pinned(&i, &tp, &p, late).await.unwrap();
    let (l, tl) = pinned(&w, "model.laya", "decision", "1.1.0", b"laya 1.1");
    assert_eq!(
        apply_pinned(&i, &tl, &l, late).await.unwrap_err(),
        InstallError::Refused(Refusal::RevocationListExpired)
    );
}
