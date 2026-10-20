#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0153/0157 end to end on loopback: `qrtz-publish` builds a signed output dir with test keys made at runtime,
//! two local servers play `modules.qrtz.ai` and its B2 mirror (a third plays Hugging Face, where the model files come
//! from, ADR 0171), and the shell's
//! package manager (`module_hosting::Packages`) refreshes, plans, installs a dependency tree under a consent in
//! parallel byte ranges, resumes, falls back to the mirror, removes, prunes, and refuses freeze, rollback and revoked
//! releases. Sideloads go through the same installer from a `.qrtzpkg` (Bozniak's rules).
#![cfg(feature = "runtime")]
use qrtz_app::module_hosting::{HttpTransport, InstallError, Op, Packages, RefreshConsent};
use qrtz_modules::index::{layout, DAY_MS};
use qrtz_modules::install::{InstallConsent, Installer};
use qrtz_modules::qrtz_lock::Source;
use qrtz_modules::sideload::{Confirmation, Refusal, Revoked, SideloadOrigin};
use qrtz_publish::dry_run::{sample, sample_hugging_face_files, NoAppleChecks};
use qrtz_publish::keys::Keys;
use qrtz_publish::{build, pack, write_root, Spec};
use qrtz_speech::testkit::Server;
use std::path::{Path, PathBuf};

struct World {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    out: PathBuf,
    keys: Keys,
    pin: String,
    now: u64,
    primary: Server,
    mirror: Server,
    /// Plays https://huggingface.co: the sample model's files at its pinned revision.
    hf: Server,
}

/// The Hugging Face path of the sample model's 20 MiB weights.
fn hf_big() -> String {
    format!(
        "qrtz/sample/resolve/{}/weights/a.bin",
        qrtz_publish::dry_run::SAMPLE_REVISION
    )
}

impl Drop for World {
    fn drop(&mut self) {
        // Installed dirs are read-only.
        let _ = qrtz_modules::fsutil::remove_tree(&self.dir);
    }
}

fn files(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, base, out);
        } else {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, std::fs::read(&p).unwrap()));
        }
    }
}

/// Copy the publisher's output dir onto a server, as `rclone sync` would.
fn serve(out: &Path, s: &Server) {
    let mut v = vec![];
    files(out, out, &mut v);
    for (p, b) in v {
        s.put(&p, b);
    }
}

async fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let out = dir.join("out");
    let keys = Keys::ephemeral();
    let now = qrtz_publish::now_ms();
    let (_, pin) = write_root(&out, &keys, None, 1, now).unwrap();
    // The model's weights are 20 MiB: three 8 MiB ranges.
    let spec = sample(&dir, true).unwrap();
    build(&out, &spec, &keys, now).unwrap();
    let (primary, mirror) = (Server::start().await, Server::start().await);
    serve(&out, &primary);
    serve(&out, &mirror);
    let hf = Server::start().await;
    for (p, b) in sample_hugging_face_files(&dir) {
        hf.put(&p, b);
    }
    World {
        _tmp: tmp,
        dir,
        out,
        keys,
        pin,
        now,
        primary,
        mirror,
        hf,
    }
}

impl World {
    fn client(&self, name: &str, hosts: &[&Server]) -> Packages {
        let bases: Vec<String> = hosts.iter().map(|s| s.base.clone()).collect();
        Packages::with(
            Installer::new(
                self.dir.join(name),
                Some(self.pin.clone()),
                Box::new(NoAppleChecks),
            ),
            self.transport(&bases),
        )
    }
    /// The given package hosts, and the loopback Hugging Face for model files.
    fn transport(&self, bases: &[String]) -> HttpTransport {
        HttpTransport::new(bases, &[("https://huggingface.co".into(), self.hf.base.clone())])
    }
    fn republish(&self, revoked: Vec<Revoked>, now: u64) {
        let spec = Spec {
            team_id: qrtz_publish::dry_run::TEAM.into(),
            packages: vec![],
            pinned_models: vec![],
            models: vec![],
            revoked,
        };
        build(&self.out, &spec, &self.keys, now).unwrap();
        serve(&self.out, &self.primary);
        serve(&self.out, &self.mirror);
    }
    fn hits(&self) -> usize {
        let a: usize = self.primary.faults.lock().unwrap().hits.values().sum();
        let b: usize = self.mirror.faults.lock().unwrap().hits.values().sum();
        a + b
    }
}

fn no_progress(_: u64, _: u64) {}

fn echo() -> Op {
    Op::Install(vec![("nano".into(), None)])
}

async fn install(c: &Packages, now: u64) -> Result<qrtz_app::module_hosting::Applied, InstallError> {
    let plan = c.plan(echo())?;
    c.apply(&plan, &plan.consent(), now, &no_progress).await
}

fn echo_lock(c: &Packages) -> qrtz_modules::qrtz_lock::Locked {
    c.installer
        .lock()
        .unwrap()
        .get("nano", "0.3.0")
        .cloned()
        .expect("echo locked")
}

#[tokio::test]
async fn publish_serve_refresh_plan_and_install_a_tree_over_localhost() {
    let w = world().await;
    let c = w.client("client", &[&w.primary, &w.mirror]);
    // No consent, no request: planning reads only the local cache.
    assert!(matches!(c.plan(echo()), Err(InstallError::NoIndex)));
    assert_eq!(w.hits(), 0, "nothing may reach a host before a consent");
    let r = c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    assert_eq!((r.timestamp_version, r.index_version), (1, 1));
    let plan = c.plan(echo()).unwrap();
    assert_eq!(
        plan.changes.len(),
        3,
        "echo, the service and the model it depends on"
    );
    assert!(plan.download_bytes > 20 * 1024 * 1024);
    // A consent for another plan (or a smaller download than this one) is refused before any blob moves.
    let low = InstallConsent {
        download_bytes: 1,
        ..plan.consent()
    };
    let before = w.hits();
    assert!(matches!(
        c.apply(&plan, &low, w.now, &no_progress).await,
        Err(InstallError::ConsentMismatch)
    ));
    assert_eq!(w.hits(), before);
    let done = c
        .apply(&plan, &plan.consent(), w.now, &no_progress)
        .await
        .unwrap();
    assert!(!done.stale_revocation);
    let root = c.installer.root();
    assert_eq!(echo_lock(&c).source, Source::Network);
    assert!(root.join("qrtz.lock").exists());
    assert!(root.join("packages/nano/0.3.0/qrtz.json").exists());
    assert!(root.join("packages/model.sample/1.0.0/weights/a.bin").exists());
    assert!(root
        .join("packages/inference.sample/1.0.0/sample.xpc/Contents/MacOS/sample")
        .exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| std::fs::metadata(root.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode("packages/nano/0.3.0/bin/qrtz-nano"),
            0o500,
            "code is copied, owner-only"
        );
        // Data is a store link (a clone on APFS, a hardlink here): never writable.
        assert_eq!(mode("packages/nano/0.3.0/qrtz.json") & 0o222, 0);
        assert_eq!(mode("packages/model.sample/1.0.0"), 0o500);
    }
    // The big model file came in closed 8 MiB ranges, from its Hugging Face URL only (ADR 0171): the package hosts
    // never hold model bytes.
    let big = hf_big();
    let model_blob = layout::blob(&qrtz_modules::sha256_hex(&vec![7u8; 20 * 1024 * 1024]));
    assert!(!w.primary.faults.lock().unwrap().hits.contains_key(&model_blob));
    let ranges: Vec<String> =
        w.hf.faults
            .lock()
            .unwrap()
            .ranges
            .iter()
            .filter(|(p, _)| *p == big)
            .map(|(_, r)| r.clone())
            .collect();
    let mut sorted = ranges.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            "bytes=0-8388607",
            "bytes=16777216-20971519",
            "bytes=8388608-16777215"
        ]
    );
    let mirror_blobs = w
        .mirror
        .faults
        .lock()
        .unwrap()
        .hits
        .keys()
        .filter(|k| k.contains("/blobs/"))
        .count();
    assert_eq!(mirror_blobs, 0);
    // A second install of the same thing fetches nothing.
    let again = c.plan(echo()).unwrap();
    assert!(again.fetch.is_empty() && again.changes.is_empty());
    // Remove and prune: the dirs and the blobs go.
    // ADR 0174: echo is bundled; neither the Extensions screen nor the `qrtz` command plans it as a package.
    for op in [
        Op::Install(vec![("echo".into(), None)]),
        Op::Remove(vec!["echo".into()]),
        Op::Update {
            names: vec!["echo".into()],
            latest: true,
        },
    ] {
        let e = c.plan(op).unwrap_err().to_string();
        assert!(e.contains("built into qrtz.app"), "{e}");
    }
    // ADR 0175 (a): the store's engine switch for echo no longer fails as "not installed" (echo has no lock
    // entry). Its own model plans nothing (Parakeet comes through speech); any other engine is refused.
    assert_eq!(
        qrtz_app::local_models::packages_needed_by("echo"),
        vec!["model.parakeet-tdt-v2-en".to_string()]
    );
    let own = c
        .plan(Op::Engine {
            extension: "echo".into(),
            model: "model.parakeet-tdt-v2-en".into(),
        })
        .unwrap();
    assert!(own.fetch.is_empty() && own.changes.is_empty());
    for model in ["model.nemotron", "model.laya", "nano"] {
        let e = c
            .plan(Op::Engine {
                extension: "echo".into(),
                model: model.into(),
            })
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("built into qrtz.app") && !e.contains("not installed"),
            "{e}"
        );
    }
    let rm = c.plan(Op::Remove(vec!["nano".into()])).unwrap();
    c.apply(&rm, &rm.consent(), w.now, &no_progress).await.unwrap();
    let p = c.prune().unwrap();
    assert_eq!(p.dirs.len(), 3);
    assert!(p.bytes > 20 * 1024 * 1024);
    assert!(!root.join("packages/nano").exists());
}

#[tokio::test]
async fn a_cut_range_is_fetched_again_and_only_it() {
    let w = world().await;
    let c = w.client("client", &[&w.primary]);
    c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    let plan = c.plan(echo()).unwrap();
    // The biggest blob is the model's weights, fetched from Hugging Face (ADR 0171).
    let big = plan.fetch.iter().max_by_key(|b| b.size).unwrap().clone();
    assert!(!big.mirrors.is_empty());
    let path = hf_big();
    w.hf.faults
        .lock()
        .unwrap()
        .truncate_once
        .insert(path.clone(), 100_000);
    let e = c
        .apply(&plan, &plan.consent(), w.now, &no_progress)
        .await
        .unwrap_err();
    assert!(matches!(e, InstallError::Network(_)), "{e:?}");
    assert!(
        c.installer.lock().unwrap().packages.is_empty(),
        "nothing is installed by a failed download"
    );
    c.apply(&plan, &plan.consent(), w.now, &no_progress)
        .await
        .unwrap();
    let hits = w.hf.faults.lock().unwrap().hits[&path];
    assert_eq!(hits, 4, "three ranges, one of them twice");
}

#[tokio::test]
async fn the_mirror_serves_when_the_primary_is_down_or_corrupt() {
    let w = world().await;
    let empty = Server::start().await;
    let c = w.client("client", &[&empty, &w.mirror]);
    c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    install(&c, w.now).await.unwrap();
    // The primary is back but serves a corrupted blob: the mirror's copy is used, checked against the index.
    let c2 = w.client("client2", &[&w.primary, &w.mirror]);
    c2.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    let plan = c2.plan(echo()).unwrap();
    // A package blob (model files never come from the package hosts, ADR 0171).
    let small = plan
        .fetch
        .iter()
        .filter(|b| b.mirrors.is_empty())
        .min_by_key(|b| b.size)
        .unwrap();
    let blob = layout::blob(&small.sha256);
    w.primary.faults.lock().unwrap().corrupt.insert(blob.clone());
    c2.apply(&plan, &plan.consent(), w.now, &no_progress)
        .await
        .unwrap();
    assert!(w.mirror.faults.lock().unwrap().hits.contains_key(&blob));
}

#[tokio::test]
async fn a_model_file_falls_back_to_its_pinned_hugging_face_url_and_only_that_host() {
    use qrtz_modules::models::{ModelFile, ModelSource};
    let w = world().await;
    let hf = Server::start().await;
    let src = ModelSource {
        package: "model.parakeet-tdt-v2-en".into(),
        version: "1.0.0".into(),
        task: "asr".into(),
        repo: "org/parakeet".into(),
        revision: "e".repeat(40),
        license: String::new(),
        files: vec![ModelFile {
            remote: "Encoder.mlmodelc/weights/weight.bin".into(),
            path: "Encoder.mlmodelc/weights/weight.bin".into(),
            size: 6,
            sha256: qrtz_modules::sha256_hex(b"weight"),
        }],
    };
    hf.put(
        &format!(
            "org/parakeet/resolve/{}/Encoder.mlmodelc/weights/weight.bin",
            "e".repeat(40)
        ),
        b"weight".to_vec(),
    );
    // The package hosts don't have it (yet): the signed tree's Hugging Face URL serves it, by the same sha256.
    let c = Packages::with(
        Installer::new(w.dir.join("c"), Some(w.pin.clone()), Box::new(NoAppleChecks)),
        HttpTransport::new(
            std::slice::from_ref(&w.primary.base),
            &[("https://huggingface.co".into(), hf.base.clone())],
        ),
    );
    let plan = c.plan(Op::Pinned(vec![src.pinned("ABCDE12345")])).unwrap();
    c.apply(&plan, &plan.consent(), w.now, &no_progress)
        .await
        .unwrap();
    assert!(c
        .installer
        .root()
        .join("packages/model.parakeet-tdt-v2-en/1.0.0/Encoder.mlmodelc/weights/weight.bin")
        .exists());
    // A URL under any other host is refused by the transport, whatever the tree says.
    let only_hf = HttpTransport::new(&[], &[("https://huggingface.co".into(), hf.base.clone())]);
    let e = qrtz_modules::fetch::Transport::get_range(
        &only_hf,
        &qrtz_modules::fetch::Source::Url("https://huggingface.co.evil.example/x".into()),
        &"0".repeat(64),
        0,
        1,
    )
    .await
    .unwrap_err();
    assert!(e.contains("not under a pinned model mirror host"), "{e}");
}

#[tokio::test]
async fn a_frozen_timestamp_is_refused_and_installs_stop() {
    let w = world().await;
    let c = w.client("client", &[&w.primary, &w.mirror]);
    c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    // Nine days on, both hosts still serve the week-old timestamp (blocked re-signing, or an attacker replaying
    // it to hide a revocation): the refresh fails, the state isn't moved, and network installs stop.
    let later = w.now + 9 * DAY_MS;
    let e = c.refresh(&RefreshConsent::user_asked(), later).await.unwrap_err();
    assert!(e.to_string().contains("expired"), "{e}");
    let plan = c.plan(echo()).unwrap();
    let e = c
        .apply(&plan, &plan.consent(), later, &no_progress)
        .await
        .unwrap_err();
    assert_eq!(e, InstallError::Refused(Refusal::RevocationListExpired));
    // Winding the clock back doesn't help: expiry is measured against the high-water mark.
    let t = c.installer.trust().unwrap().unwrap();
    assert!(t.state.clock_hwm_ms >= w.now);
}

#[tokio::test]
async fn an_older_timestamp_or_index_is_a_rollback() {
    let w = world().await;
    let old_ts = std::fs::read(w.out.join(layout::timestamp())).unwrap();
    let old_sig = std::fs::read(w.out.join(layout::sig(&layout::timestamp()))).unwrap();
    w.republish(vec![], w.now + 1000);
    let c = w.client("client", &[&w.primary, &w.mirror]);
    let r = c
        .refresh(&RefreshConsent::user_asked(), w.now + 2000)
        .await
        .unwrap();
    assert_eq!(r.index_version, 2);
    // Both hosts go back to version 1 (still unexpired and validly signed).
    for s in [&w.primary, &w.mirror] {
        s.put(&layout::timestamp(), old_ts.clone());
        s.put(&layout::sig(&layout::timestamp()), old_sig.clone());
    }
    let e = c
        .refresh(&RefreshConsent::user_asked(), w.now + 3000)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("older than the stored"), "{e}");
    assert_eq!(
        c.installer.trust().unwrap().unwrap().state.index.unwrap().version,
        2
    );
}

#[tokio::test]
async fn a_revocation_disables_the_installed_extension_and_refuses_new_installs() {
    let w = world().await;
    let c = w.client("client", &[&w.primary, &w.mirror]);
    c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    install(&c, w.now).await.unwrap();
    let pulled = Revoked {
        package: "nano".into(),
        versions: vec!["0.3.0".into()],
        reason: "pulled: crash on launch".into(),
    };
    w.republish(vec![pulled], w.now + 1000);
    let r = c
        .refresh(&RefreshConsent::user_asked(), w.now + 2000)
        .await
        .unwrap();
    assert_eq!(r.notices.len(), 1);
    assert_eq!(r.notices[0].module, "nano");
    assert_eq!(echo_lock(&c).disabled.as_deref(), Some("pulled: crash on launch"));
    // A fresh client can't install it either: resolution skips revoked versions.
    let c2 = w.client("client2", &[&w.primary]);
    c2.refresh(&RefreshConsent::user_asked(), w.now + 2000)
        .await
        .unwrap();
    assert!(c2.plan(echo()).is_err(), "no unrevoked echo");
    assert!(c2
        .plan(Op::Install(vec![("nano".into(), Some("=0.3.0".into()))]))
        .is_err());
}

#[tokio::test]
async fn sideload_follows_bozniaks_rules() {
    let w = world().await;
    let pkg = w.dir.join("echo-0.3.0.qrtzpkg");
    pack(&w.out, "nano", "0.3.0", &pkg).unwrap();
    let late = w.now + 20 * DAY_MS;

    // Rule 4: a file from a watched folder is refused; a user-picked one asks first, showing the list's age.
    let c = w.client("air-gapped", &[]);
    let e = c
        .installer
        .sideload(&pkg, SideloadOrigin::WatchedFolder, None, late)
        .await
        .unwrap_err();
    assert_eq!(e, InstallError::Refused(Refusal::NotUserInitiated));
    let InstallError::Refused(Refusal::NeedsConfirmation(p)) = c
        .installer
        .sideload(&pkg, SideloadOrigin::UserPickedFile, None, late)
        .await
        .unwrap_err()
    else {
        panic!("no prompt")
    };
    assert!(p.revocation_expired);
    assert!(p.revocation_age_ms.unwrap() >= 20 * DAY_MS);
    assert!(
        c.installer.lock().unwrap().packages.is_empty(),
        "the prompt installs nothing"
    );
    // Rule 2/6: an expired carried list still installs after the yes, marked stale-revocation.
    let yes = Confirmation {
        package_sha256: p.package_sha256.clone(),
        revocation_issued_ms: p.revocation_issued_ms,
    };
    let done = c
        .installer
        .sideload(&pkg, SideloadOrigin::UserPickedFile, Some(&yes), late)
        .await
        .unwrap();
    assert!(done.stale_revocation);
    assert_eq!(done.changes.len(), 3, "echo with its service and model");
    assert_eq!(echo_lock(&c).source, Source::Sideload);

    // Rule 3: a Mac that already has a newer list refuses the file's older one as a rollback.
    w.republish(vec![], w.now + 1000);
    let fresh = w.client("online", &[&w.primary]);
    fresh
        .refresh(&RefreshConsent::user_asked(), w.now + 2000)
        .await
        .unwrap();
    let e = fresh
        .installer
        .sideload(&pkg, SideloadOrigin::UserPickedFile, Some(&yes), w.now + 2000)
        .await
        .unwrap_err();
    assert!(
        matches!(e, InstallError::Refused(Refusal::RevocationRollback { .. })),
        "{e:?}"
    );

    // Rule 2: a stored list that revokes it refuses the sideload after the yes, with no override.
    let pulled = Revoked {
        package: "nano".into(),
        versions: vec![],
        reason: "pulled".into(),
    };
    // Issued a day before `late`, so it is still fresh on the air-gapped Mac, whose clock has seen `late`.
    w.republish(vec![pulled], late - DAY_MS);
    let pkg2 = w.dir.join("echo-0.3.0-b.qrtzpkg");
    pack(&w.out, "nano", "0.3.0", &pkg2).unwrap();
    let revoker = w.client("revoker", &[&w.primary]);
    revoker
        .refresh(&RefreshConsent::user_asked(), late)
        .await
        .unwrap();
    // Rule 2 comes before the prompt: a revoked file is refused outright, so there is nothing to say yes to.
    let e = revoker
        .installer
        .sideload(&pkg2, SideloadOrigin::UserPickedFile, None, late)
        .await
        .unwrap_err();
    assert!(
        matches!(e, InstallError::Refused(Refusal::Revoked { .. })),
        "{e:?}"
    );

    // Rules 3 and 6: the air-gapped Mac takes the newer list another file carries. It is fresh and revokes the
    // echo it sideloaded earlier, so that echo is disabled at once, and this file is refused.
    assert!(echo_lock(&c).disabled.is_none());
    let e = c
        .installer
        .sideload(&pkg2, SideloadOrigin::UserPickedFile, None, late)
        .await
        .unwrap_err();
    assert!(
        matches!(e, InstallError::Refused(Refusal::Revoked { .. })),
        "{e:?}"
    );
    assert!(echo_lock(&c).disabled.is_some(), "rule 6 on adoption");
    // And the old file can't take it back (its list is older now).
    let e = c
        .installer
        .sideload(&pkg, SideloadOrigin::UserPickedFile, Some(&yes), late)
        .await
        .unwrap_err();
    assert!(
        matches!(e, InstallError::Refused(Refusal::RevocationRollback { .. })),
        "{e:?}"
    );
}

#[tokio::test]
async fn a_root_rotation_needs_the_old_and_the_new_root_keys() {
    let w = world().await;
    let c = w.client("client", &[&w.primary]);
    c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    // A root 2 signed only by the new keys is refused at publish time, and by the client if served anyway.
    let mut next = Keys::ephemeral();
    next.timestamp = None;
    next.index = None;
    next.timestamp_public = w.keys.timestamp_public;
    next.index_public = w.keys.index_public;
    assert!(write_root(&w.out, &next, None, 1, w.now)
        .unwrap_err()
        .contains("--prev-keys"));
    let (n, _) = write_root(&w.out, &next, Some(&w.keys), 1, w.now + 500).unwrap();
    assert_eq!(n, 2);
    w.republish(vec![], w.now + 1000);
    c.refresh(&RefreshConsent::user_asked(), w.now + 2000)
        .await
        .unwrap();
    assert_eq!(c.installer.trust().unwrap().unwrap().root.version, 2);
}

#[tokio::test]
async fn an_unpinned_root_is_refused() {
    let w = world().await;
    let c = Packages::with(
        Installer::new(w.dir.join("c"), Some("00".repeat(32)), Box::new(NoAppleChecks)),
        w.transport(std::slice::from_ref(&w.primary.base)),
    );
    let e = c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap_err();
    assert!(e.to_string().contains("pinned"), "{e}");
    let none = Packages::with(
        Installer::new(w.dir.join("d"), None, Box::new(NoAppleChecks)),
        w.transport(std::slice::from_ref(&w.primary.base)),
    );
    let before = w.hits();
    let e = none
        .refresh(&RefreshConsent::user_asked(), w.now)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no root is pinned"), "{e}");
    assert_eq!(w.hits(), before, "no pin, no request");
}

#[tokio::test]
async fn the_qrtz_cli_drives_the_same_installer() {
    use qrtz_app::pm_cli::{parse, run, Io};
    let w = world().await;
    let c = w.client("cli", &[&w.primary]);
    let mut out: Vec<u8> = vec![];
    let mut answers = vec![false];
    let mut asked = vec![];
    async fn go(
        c: &Packages,
        line: &str,
        out: &mut Vec<u8>,
        answers: &mut Vec<bool>,
        asked: &mut Vec<String>,
    ) -> Result<(), String> {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        let (cmd, f) = parse(&args)?;
        let mut ask = |q: &str| {
            asked.push(q.to_owned());
            answers.pop().unwrap_or(false)
        };
        let mut io = Io { out, ask: &mut ask };
        run(c, cmd, &f, &mut io).await
    }
    // Asked, said no: nothing changes.
    go(&c, "install nano", &mut out, &mut answers, &mut asked)
        .await
        .unwrap();
    assert_eq!(asked, ["Proceed?"]);
    assert!(c.installer.lock().unwrap().packages.is_empty());
    let text = String::from_utf8_lossy(&out).to_string();
    assert!(
        text.contains("+ nano@0.3.0")
            && text.contains("+ model.sample@1.0.0")
            && text.contains("nothing changed"),
        "{text}"
    );
    // --dry-run shows and stops; -y installs.
    go(&c, "install nano --dry-run", &mut out, &mut answers, &mut asked)
        .await
        .unwrap();
    assert!(c.installer.lock().unwrap().packages.is_empty());
    go(&c, "install nano -y", &mut out, &mut answers, &mut asked)
        .await
        .unwrap();
    assert_eq!(c.installer.lock().unwrap().packages.len(), 3);
    out.clear();
    go(&c, "ls", &mut out, &mut answers, &mut asked).await.unwrap();
    let ls = String::from_utf8_lossy(&out).to_string();
    assert!(
        ls.contains("nano@0.3.0  Extension") && ls.contains("requested ^0.3.0"),
        "{ls}"
    );
    assert!(
        ls.contains("model.sample@1.0.0  Model") && ls.contains("for nano"),
        "{ls}"
    );
    // Frozen and offline: nothing to do, no request.
    let before = w.hits();
    go(&c, "install --frozen -y", &mut out, &mut answers, &mut asked)
        .await
        .unwrap();
    assert_eq!(w.hits(), before);
    go(&c, "remove nano -y", &mut out, &mut answers, &mut asked)
        .await
        .unwrap();
    assert_eq!(w.hits(), before, "remove is local");
    out.clear();
    go(&c, "prune", &mut out, &mut answers, &mut asked).await.unwrap();
    assert!(String::from_utf8_lossy(&out).starts_with("pruned 3 package dirs"));
}

#[tokio::test]
async fn the_extensions_screen_applies_only_the_plan_it_was_shown() {
    use qrtz_app::packages_ipc::PackagesIpc;
    use qrtz_app::ports::PackagesPort;
    use serde_json::json;
    let w = world().await;
    let ipc = PackagesIpc::new(w.client("ui", &[&w.primary]));
    let c = qrtz_ipc::contract();
    let check = |ch: &str, v: &serde_json::Value| {
        assert!(c.channels[ch].response.is_valid(v), "{ch}: {v}");
    };
    let s = ipc.call("packages:list", json!(null)).await.unwrap();
    check("packages:list", &s);
    assert_eq!(
        s,
        json!({"packages": [], "index": null, "available": [],
               "reclaimable": {"dirs": 0, "blobs": 0, "bytes": 0}, "pendingRestart": []})
    );
    assert_eq!(w.hits(), 0, "listing is local");
    let s = ipc.call("packages:refresh", json!(null)).await.unwrap();
    assert_eq!(s["index"]["version"], 1);
    let plan = ipc
        .call("packages:plan", json!({"op": "install", "names": ["nano"]}))
        .await
        .unwrap();
    check("packages:plan", &plan);
    assert_eq!(plan["changes"].as_array().unwrap().len(), 3);
    // Another hash, or a smaller download than shown: refused, nothing moves.
    let before = w.hits();
    let e = ipc
        .call(
            "packages:apply",
            json!({"planSha256": "0".repeat(64), "downloadBytes": plan["downloadBytes"]}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, qrtz_ipc::ErrorCode::Validation);
    let e = ipc
        .call(
            "packages:apply",
            json!({"planSha256": plan["planSha256"], "downloadBytes": 1}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, qrtz_ipc::ErrorCode::Validation);
    assert_eq!(w.hits(), before);
    let a = ipc
        .call(
            "packages:apply",
            json!({"planSha256": plan["planSha256"], "downloadBytes": plan["downloadBytes"]}),
        )
        .await
        .unwrap();
    check("packages:apply", &a);
    // A plan is used once.
    assert!(ipc
        .call(
            "packages:apply",
            json!({"planSha256": plan["planSha256"], "downloadBytes": plan["downloadBytes"]})
        )
        .await
        .is_err());
    let s = ipc.call("packages:list", json!(null)).await.unwrap();
    check("packages:list", &s);
    let kinds: Vec<&str> = s["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["service", "model", "extension"]);
    let plan = ipc
        .call("packages:plan", json!({"op": "remove", "names": ["nano"]}))
        .await
        .unwrap();
    ipc.call(
        "packages:apply",
        json!({"planSha256": plan["planSha256"], "downloadBytes": 0}),
    )
    .await
    .unwrap();
    let p = ipc.call("packages:prune", json!(null)).await.unwrap();
    check("packages:prune", &p);
    assert_eq!(p["dirs"], 3);
}

/// Audit P1: "check for updates" refreshes the signed index only when asked and installs nothing. The sample index
/// lists no `qrtz` shell release yet; an unconfigured transport (today's pinned hosts) asks nothing at all.
#[tokio::test]
async fn check_for_updates_refreshes_the_signed_index_and_installs_nothing() {
    use qrtz_app::packages_ipc::PackagesIpc;
    use qrtz_app::ports::PackagesPort;
    use serde_json::json;
    let w = world().await;
    let ipc = PackagesIpc::new(w.client("ui", &[&w.primary])).with_app_version("0.3.0");
    let c = qrtz_ipc::contract();
    assert_eq!(w.hits(), 0);
    let r = ipc.call("packages:check-update", json!(null)).await.unwrap();
    assert!(c.channels["packages:check-update"].response.is_valid(&r), "{r}");
    assert_eq!(
        r,
        json!({"current": "0.3.0", "state": "none-listed", "latest": null, "sizeBytes": null})
    );
    assert!(w.hits() > 0, "the check refreshed the index");
    let s = ipc.call("packages:list", json!(null)).await.unwrap();
    assert_eq!(s["packages"], json!([]), "nothing installed");
    assert_eq!(s["index"]["version"], 1);

    let before = w.hits();
    let off = Packages::with(
        Installer::new(w.dir.join("off"), Some(w.pin.clone()), Box::new(NoAppleChecks)),
        w.transport(std::slice::from_ref(&w.primary.base)).unconfigured(),
    );
    let r = PackagesIpc::new(off)
        .call("packages:check-update", json!(null))
        .await
        .unwrap();
    assert_eq!(r["state"], "not-set-up");
    assert_eq!(w.hits(), before, "an unconfigured host gets no request");
}

/// N189 (ADR 0210 follow-up): a refresh stuck on a host that accepts and never answers (a black-holed network, a
/// captive portal) holds no install lock while it waits, so `packages:apply` of a package already in the store
/// installs at once instead of failing with "another install is running" for a minute or more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_stuck_on_a_silent_host_blocks_no_install() {
    use qrtz_app::packages_ipc::PackagesIpc;
    use qrtz_app::ports::PackagesPort;
    use serde_json::json;
    let w = world().await;
    let ipc = PackagesIpc::new(w.client("ui", &[&w.primary]));
    ipc.call("packages:refresh", json!(null)).await.unwrap();
    let apply = |plan: serde_json::Value| {
        let ipc = &ipc;
        async move {
            ipc.call(
                "packages:apply",
                json!({"planSha256": plan["planSha256"], "downloadBytes": plan["downloadBytes"]}),
            )
            .await
        }
    };
    let install = json!({"op": "install", "names": ["nano"]});
    let plan = ipc.call("packages:plan", install.clone()).await.unwrap();
    apply(plan).await.unwrap();
    let plan = ipc
        .call("packages:plan", json!({"op": "remove", "names": ["nano"]}))
        .await
        .unwrap();
    apply(plan).await.unwrap();
    // Its blobs stay in the store until a prune: installing it again fetches nothing.
    let plan = ipc.call("packages:plan", install).await.unwrap();
    assert_eq!(plan["downloadBytes"], 0, "{plan}");

    // The only host: accepts every connection and never sends a byte.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", silent.local_addr().unwrap().port());
    let (connected, accepted) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut held = vec![];
        for s in silent.incoming().flatten() {
            held.push(s);
            let _ = connected.send(());
        }
    });
    let stuck = std::sync::Arc::new(Packages::with(
        Installer::new(w.dir.join("ui"), Some(w.pin.clone()), Box::new(NoAppleChecks)),
        w.transport(&[base]),
    ));
    let now = w.now;
    let refresh = tokio::spawn({
        let stuck = stuck.clone();
        async move { stuck.refresh(&RefreshConsent::user_asked(), now).await }
    });
    tokio::task::spawn_blocking(move || accepted.recv_timeout(std::time::Duration::from_secs(10)))
        .await
        .unwrap()
        .expect("the refresh reached the silent host");

    let a = apply(plan).await.expect("an install while a refresh waits");
    assert_eq!(a["changes"].as_array().map(Vec::len), Some(3), "{a}");
    assert!(
        !refresh.is_finished(),
        "the refresh is still waiting on the silent host"
    );
    assert!(echo_lock(&stuck).disabled.is_none());
    refresh.abort();
}

/// Plays a host through `inner`; the moment this refresh asks for the index (its timestamp already verified against
/// its snapshot of the trust, before its commit), another client refreshes the same data dir to completion.
struct MidRefresh {
    inner: HttpTransport,
    other: std::sync::Mutex<Option<Packages>>,
    now: u64,
}

#[async_trait::async_trait]
impl qrtz_modules::fetch::Transport for MidRefresh {
    fn hosts(&self) -> usize {
        qrtz_modules::fetch::Transport::hosts(&self.inner)
    }
    async fn get(&self, host: usize, path: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
        if path.starts_with("v1/index/") {
            let other = self.other.lock().unwrap().take();
            if let Some(o) = other {
                o.refresh(&RefreshConsent::user_asked(), self.now)
                    .await
                    .expect("the other refresh isn't blocked by this one");
            }
        }
        qrtz_modules::fetch::Transport::get(&self.inner, host, path, max).await
    }
    async fn get_range(
        &self,
        src: &qrtz_modules::fetch::Source,
        sha256: &str,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>, String> {
        self.inner.get_range(src, sha256, start, len).await
    }
}

/// Index v2 (revoking nano@0.3.0) on its own server, v3 (revoking nothing) on the primary and the mirror.
async fn two_versions(w: &World) -> (Server, Revoked) {
    let pulled = Revoked {
        package: "nano".into(),
        versions: vec!["0.3.0".into()],
        reason: "pulled: crash on launch".into(),
    };
    w.republish(vec![pulled.clone()], w.now + 1000);
    let v2 = Server::start().await;
    serve(&w.out, &v2);
    w.republish(vec![], w.now + 2000);
    (v2, pulled)
}

/// N189: the stored trust moves while a refresh fetches (another refresh commits v2): the commit re-verifies the
/// fetched v3 against it and builds on it, so v2's revocation is kept, not overwritten by the stale snapshot.
#[tokio::test]
async fn trust_that_moves_mid_refresh_is_reverified_and_kept() {
    let w = world().await;
    let (v2, pulled) = two_versions(&w).await;
    let now = w.now + 3000;
    let mid = MidRefresh {
        inner: w.transport(std::slice::from_ref(&w.primary.base)),
        other: std::sync::Mutex::new(Some(w.client("c", &[&v2]))),
        now,
    };
    let a = Installer::new(w.dir.join("c"), Some(w.pin.clone()), Box::new(NoAppleChecks));
    let r = a.refresh(&mid, &RefreshConsent::user_asked(), now).await.unwrap();
    assert!(
        mid.other.lock().unwrap().is_none(),
        "the other refresh ran mid-way"
    );
    assert_eq!(r.index_version, 3);
    let t = a.trust().unwrap().unwrap();
    assert_eq!(t.state.index.as_ref().unwrap().version, 3);
    assert!(
        t.revocation()
            .unwrap()
            .revoked
            .iter()
            .any(|x| x.package == pulled.package),
        "v2's revocation accumulates under v3"
    );
    let (_, v) = a.cached_index().unwrap();
    assert_eq!(v.index.version, 3);
}

/// N189: a refresh that fetched v2 while another committed v3 is refused at its commit (no rollback, retried once
/// from the new trust and refused again), and v3 stays stored and cached.
#[tokio::test]
async fn an_older_pair_committed_after_a_newer_one_is_a_rollback() {
    let w = world().await;
    let (v2, _) = two_versions(&w).await;
    let now = w.now + 3000;
    let mid = MidRefresh {
        inner: w.transport(std::slice::from_ref(&v2.base)),
        other: std::sync::Mutex::new(Some(w.client("c", &[&w.primary]))),
        now,
    };
    let a = Installer::new(w.dir.join("c"), Some(w.pin.clone()), Box::new(NoAppleChecks));
    let e = a
        .refresh(&mid, &RefreshConsent::user_asked(), now)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("older than the stored"), "{e}");
    assert!(
        mid.other.lock().unwrap().is_none(),
        "the other refresh ran mid-way"
    );
    let t = a.trust().unwrap().unwrap();
    assert_eq!(t.state.index.as_ref().unwrap().version, 3);
    assert_eq!(t.state.timestamp.as_ref().unwrap().version, 3);
    let (_, v) = a.cached_index().unwrap();
    assert_eq!(v.index.version, 3);
}
