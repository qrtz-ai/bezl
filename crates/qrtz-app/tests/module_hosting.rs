//! ADR 0153 end to end on loopback: `qrtz-publish` builds a signed output dir with test keys made at runtime, two
//! local servers play `modules.qrtz.ai` and its B2 mirror, and the shell's installer (`module_hosting`) refreshes,
//! plans, installs under a consent, resumes, falls back to the mirror, and refuses freeze, rollback and revoked
//! releases. Sideloads go through the same installer from a `.qrtzpkg` (Bozniak's rules).
#![cfg(feature = "runtime")]
use qrtz_app::module_hosting::{InstallError, ModuleHosting, RefreshConsent};
use qrtz_modules::index::{layout, DAY_MS};
use qrtz_modules::install::InstallConsent;
use qrtz_modules::lock::Source;
use qrtz_modules::sideload::{Confirmation, Refusal, Revoked, SideloadOrigin};
use qrtz_publish::dry_run::{sample, NoAppleChecks};
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
    let spec = sample(&dir, false).unwrap();
    build(&out, &spec, &keys, now).unwrap();
    let (primary, mirror) = (Server::start().await, Server::start().await);
    serve(&out, &primary);
    serve(&out, &mirror);
    World {
        _tmp: tmp,
        dir,
        out,
        keys,
        pin,
        now,
        primary,
        mirror,
    }
}

impl World {
    fn client(&self, name: &str, hosts: &[&Server]) -> ModuleHosting {
        let bases: Vec<String> = hosts.iter().map(|s| s.base.clone()).collect();
        ModuleHosting::with(
            self.dir.join(name),
            &bases,
            Some(self.pin.clone()),
            Box::new(NoAppleChecks),
        )
    }
    fn republish(&self, revoked: Vec<Revoked>, now: u64) {
        let spec = Spec {
            team_id: qrtz_publish::dry_run::TEAM.into(),
            packages: vec![],
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

/// Ask without a yes; return the yes the prompt allows.
async fn ask(c: &ModuleHosting, f: &Path, now: u64) -> Confirmation {
    match c
        .installer
        .sideload(f, SideloadOrigin::UserPickedFile, None, now)
        .await
    {
        Err(InstallError::Refused(Refusal::NeedsConfirmation(p))) => Confirmation {
            package_sha256: p.package_sha256,
            revocation_issued_ms: p.revocation_issued_ms,
        },
        other => panic!("no prompt: {other:?}"),
    }
}

#[tokio::test]
async fn publish_serve_refresh_plan_and_install_over_localhost() {
    let w = world().await;
    let c = w.client("client", &[&w.primary, &w.mirror]);
    // No consent, no request: planning reads only the local cache.
    assert!(matches!(c.plan("echo", None), Err(InstallError::NoIndex)));
    assert_eq!(w.hits(), 0, "nothing may reach a host before a consent");
    let r = c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    assert_eq!((r.timestamp_version, r.index_version), (1, 1));
    let plan = c.plan("echo", None).unwrap();
    assert_eq!(plan.version, "0.3.0");
    assert_eq!(plan.packages.len(), 2, "echo and the model set it pins");
    assert!(plan.download_bytes > 300_000);
    // A consent for another plan (or a smaller download than this one) is refused before any blob moves.
    let low = InstallConsent {
        download_bytes: 1,
        ..plan.consent()
    };
    let before = w.hits();
    assert!(matches!(
        c.install(&low, w.now, &no_progress).await,
        Err(InstallError::ConsentMismatch)
    ));
    assert_eq!(w.hits(), before);
    let done = c.install(&plan.consent(), w.now, &no_progress).await.unwrap();
    assert_eq!(
        (done.module.as_str(), done.version.as_str(), done.stale_revocation),
        ("echo", "0.3.0", false)
    );
    let root = c.installer.root();
    let lock = c.installer.lock().unwrap();
    assert_eq!(lock.modules["echo"].source, Source::Network);
    assert!(lock.package("model:sample", "1.0.0").is_some());
    assert!(root.join("modules/echo/0.3.0/module.json").exists());
    assert!(root.join("packages/model/sample/1.0.0/weights/a.bin").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| std::fs::metadata(root.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode("modules/echo/0.3.0/qrtz-echo.app/Contents/MacOS/qrtz-echo"),
            0o500
        );
        assert_eq!(mode("modules/echo/0.3.0/module.json"), 0o400);
        assert_eq!(mode("packages/model/sample/1.0.0"), 0o500);
    }
    // Blobs only came from the primary; the mirror wasn't needed.
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
    let again = c.plan("echo", Some("0.3.0")).unwrap();
    assert!(again.fetch.is_empty() && again.packages.iter().all(|p| p.installed));
}

#[tokio::test]
async fn a_cut_download_resumes_with_a_range_request() {
    let w = world().await;
    let c = w.client("client", &[&w.primary]);
    c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    let plan = c.plan("echo", None).unwrap();
    let big = plan.fetch.iter().max_by_key(|b| b.size).unwrap().clone();
    let path = layout::blob(&big.sha256);
    w.primary
        .faults
        .lock()
        .unwrap()
        .truncate_once
        .insert(path.clone(), 100_000);
    let e = c.install(&plan.consent(), w.now, &no_progress).await.unwrap_err();
    assert!(matches!(e, InstallError::Network(_)), "{e:?}");
    assert!(
        c.installer.lock().unwrap().modules.is_empty(),
        "nothing is installed by a failed download"
    );
    let plan = c.plan("echo", None).unwrap();
    c.install(&plan.consent(), w.now, &no_progress).await.unwrap();
    let ranges = w.primary.faults.lock().unwrap().ranges.clone();
    assert!(
        ranges.iter().any(|(p, r)| *p == path && r == "bytes=100000-"),
        "{ranges:?}"
    );
}

#[tokio::test]
async fn the_mirror_serves_when_the_primary_is_down_or_corrupt() {
    let w = world().await;
    let empty = Server::start().await;
    let c = w.client("client", &[&empty, &w.mirror]);
    c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    // The primary is back but serves a corrupted blob: the mirror's copy is used, checked against the index.
    let c2 = w.client("client2", &[&w.primary, &w.mirror]);
    c2.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    let plan = c2.plan("echo", None).unwrap();
    let blob = layout::blob(&plan.fetch[0].sha256);
    w.primary.faults.lock().unwrap().corrupt.insert(blob.clone());
    c2.install(&plan.consent(), w.now, &no_progress).await.unwrap();
    assert!(w.mirror.faults.lock().unwrap().hits.contains_key(&blob));
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
    let plan = c.plan("echo", None).unwrap();
    let e = c.install(&plan.consent(), later, &no_progress).await.unwrap_err();
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
async fn a_revocation_disables_the_installed_module_and_refuses_new_installs() {
    let w = world().await;
    let c = w.client("client", &[&w.primary, &w.mirror]);
    c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap();
    let plan = c.plan("echo", None).unwrap();
    c.install(&plan.consent(), w.now, &no_progress).await.unwrap();
    let pulled = Revoked {
        package: "echo".into(),
        versions: vec!["0.3.0".into()],
        reason: "pulled: crash on launch".into(),
    };
    w.republish(vec![pulled], w.now + 1000);
    let r = c
        .refresh(&RefreshConsent::user_asked(), w.now + 2000)
        .await
        .unwrap();
    assert_eq!(r.notices.len(), 1);
    assert_eq!(r.notices[0].module, "echo");
    assert_eq!(
        c.installer.lock().unwrap().modules["echo"].disabled.as_deref(),
        Some("pulled: crash on launch")
    );
    // A fresh client can't install it either.
    let c2 = w.client("client2", &[&w.primary]);
    c2.refresh(&RefreshConsent::user_asked(), w.now + 2000)
        .await
        .unwrap();
    assert!(c2.plan("echo", None).is_err(), "latest skips revoked versions");
    let plan = c2.plan("echo", Some("0.3.0")).unwrap();
    let e = c2
        .install(&plan.consent(), w.now + 2000, &no_progress)
        .await
        .unwrap_err();
    assert!(
        matches!(e, InstallError::Refused(Refusal::Revoked { .. })),
        "{e:?}"
    );
}

#[tokio::test]
async fn sideload_follows_bozniaks_rules() {
    let w = world().await;
    let pkg = w.dir.join("echo-0.3.0.qrtzpkg");
    pack(&w.out, "echo", "0.3.0", &pkg).unwrap();
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
        c.installer.lock().unwrap().modules.is_empty(),
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
    assert_eq!(
        c.installer.lock().unwrap().modules["echo"].source,
        Source::Sideload
    );

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
        package: "echo".into(),
        versions: vec![],
        reason: "pulled".into(),
    };
    // Issued a day before `late`, so it is still fresh on the air-gapped Mac, whose clock has seen `late`.
    w.republish(vec![pulled], late - DAY_MS);
    let pkg2 = w.dir.join("echo-0.3.0-b.qrtzpkg");
    pack(&w.out, "echo", "0.3.0", &pkg2).unwrap();
    let revoker = w.client("revoker", &[&w.primary]);
    revoker
        .refresh(&RefreshConsent::user_asked(), late)
        .await
        .unwrap();
    let yes2 = ask(&revoker, &pkg2, late).await;
    let e = revoker
        .installer
        .sideload(&pkg2, SideloadOrigin::UserPickedFile, Some(&yes2), late)
        .await
        .unwrap_err();
    assert!(
        matches!(e, InstallError::Refused(Refusal::Revoked { .. })),
        "{e:?}"
    );

    // Rules 3 and 6: the air-gapped Mac takes the newer list another file carries. It is fresh and revokes the
    // echo it sideloaded earlier, so that echo is disabled at once, and this file is refused.
    assert!(c.installer.lock().unwrap().modules["echo"].disabled.is_none());
    let yes3 = ask(&c, &pkg2, late).await;
    assert!(
        c.installer.lock().unwrap().modules["echo"].disabled.is_some(),
        "rule 6 on adoption"
    );
    let e = c
        .installer
        .sideload(&pkg2, SideloadOrigin::UserPickedFile, Some(&yes3), late)
        .await
        .unwrap_err();
    assert!(
        matches!(e, InstallError::Refused(Refusal::Revoked { .. })),
        "{e:?}"
    );
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
    let c = ModuleHosting::with(
        w.dir.join("c"),
        std::slice::from_ref(&w.primary.base),
        Some("00".repeat(32)),
        Box::new(NoAppleChecks),
    );
    let e = c.refresh(&RefreshConsent::user_asked(), w.now).await.unwrap_err();
    assert!(e.to_string().contains("pinned"), "{e}");
    let none = ModuleHosting::with(
        w.dir.join("d"),
        std::slice::from_ref(&w.primary.base),
        None,
        Box::new(NoAppleChecks),
    );
    let before = w.hits();
    let e = none
        .refresh(&RefreshConsent::user_asked(), w.now)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no root is pinned"), "{e}");
    assert_eq!(w.hits(), before, "no pin, no request");
}
