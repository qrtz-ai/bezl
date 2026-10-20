#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0157: the package manager end to end on the real publisher output, through a directory transport (the HTTP
//! transport is qrtz-app's tests): resolution and dedupe, engine swaps, the lock, the store, prune, frozen
//! reinstalls, mirror fallback, resumable ranges, revocation and the freshness rule.
use qrtz_modules::fetch::{Source, Transport};
use qrtz_modules::index::{layout, PackageKind, DAY_MS};
use qrtz_modules::install::{Change, InstallError, Installer, Op, RefreshConsent};
use qrtz_modules::sideload::{Refusal, Revoked};
use qrtz_publish::dry_run::NoAppleChecks;
use qrtz_publish::keys::Keys;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const TEAM: &str = "ABCDE12345";
const REV: &str = "0123456789abcdef0123456789abcdef01234567";

/// Hosts are output dirs; mirror URL prefixes map to dirs.
struct Dirs {
    hosts: Vec<PathBuf>,
    urls: BTreeMap<String, PathBuf>,
    /// Ranges (blob sha prefix) that fail once.
    fail_once: Mutex<BTreeSet<String>>,
    pub ranges: Mutex<Vec<(String, u64)>>,
}

impl Dirs {
    fn new(hosts: Vec<PathBuf>) -> Self {
        Self {
            hosts,
            urls: BTreeMap::new(),
            fail_once: Mutex::default(),
            ranges: Mutex::default(),
        }
    }
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
                    .filter(|(p, _)| u.starts_with(*p))
                    .max_by_key(|(p, _)| p.len())
                    .ok_or("unknown mirror")?;
                dir.join(&u[pre.len()..])
            }
        };
        let key = format!("{}@{start}", &sha[..8]);
        if self.fail_once.lock().unwrap().remove(&key) {
            return Err("connection reset".into());
        }
        self.ranges.lock().unwrap().push((sha.to_owned(), start));
        let b = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let end = start + len;
        if end as usize > b.len() {
            return Err("range outside".into());
        }
        Ok(b[start as usize..end as usize].to_vec())
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

    fn model(&mut self, name: &str, v: &str, task: &str, bytes: &[u8]) -> PathBuf {
        let d = self.dir();
        write(
            &d.join("qrtz.json"),
            &serde_json::to_vec(
                // ADR 0171: a model's files come from Hugging Face at a pinned commit (here: this dir).
                &json!({"name": name, "version": v, "kind": "model",
                    "model": {"task": task, "upstream": format!("qrtz-test/m{}", self.n), "revision": REV}}),
            )
            .unwrap(),
            false,
        );
        write(&d.join("weights.bin"), bytes, false);
        // What Hugging Face serves for it ([`World::hosts`] maps the URL prefix here).
        write(
            &self
                .base
                .join(format!("hf/qrtz-test/m{}/resolve/{REV}/weights.bin", self.n)),
            bytes,
            false,
        );
        d
    }

    fn service(&mut self, name: &str, v: &str, models: &[&str]) -> PathBuf {
        let d = self.dir();
        write(
            &d.join("qrtz.json"),
            &serde_json::to_vec(&json!({"name": name, "version": v, "kind": "service",
                "service": {"bundle": "s.xpc", "models": models}}))
            .unwrap(),
            false,
        );
        write(
            &d.join("s.xpc/Contents/MacOS/s"),
            format!("svc {name} {v}").as_bytes(),
            true,
        );
        d
    }

    fn ext(&mut self, name: &str, v: &str, deps: serde_json::Value) -> PathBuf {
        let d = self.dir();
        write(
            &d.join("qrtz.json"),
            &serde_json::to_vec(
                &json!({"name": name, "version": v, "kind": "extension", "dependencies": deps,
                "extension": {"entry": "bin/helper", "protocol": 1, "minApp": "0.3.0"}}),
            )
            .unwrap(),
            false,
        );
        write(&d.join("bin/helper"), format!("{name} {v}").as_bytes(), true);
        d
    }

    fn publish(&self, dirs: Vec<PathBuf>, revoked: Vec<Revoked>, at: u64) {
        let spec = qrtz_publish::Spec {
            team_id: TEAM.into(),
            packages: dirs
                .into_iter()
                .map(qrtz_publish::SpecPackage::fake_notarized)
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
        let mut d = Dirs::new(vec![self.out.clone()]);
        d.urls.insert(
            "https://huggingface.co/qrtz-test/".into(),
            self.base.join("hf/qrtz-test"),
        );
        d
    }
}

/// The standard catalogue: echo needs inference.voice + parakeet, nano needs inference.voice + nemotron.
fn catalogue(w: &mut World) {
    let p = w.model("model.parakeet-tdt-v2-en", "1.0.0", "asr", &[1u8; 4000]);
    let n = w.model("model.nemotron", "1.0.0", "asr", &[2u8; 5000]);
    let v = w.service(
        "inference.voice",
        "1.0.0",
        &["model.parakeet-tdt-v2-en", "model.nemotron"],
    );
    let e = w.ext(
        "echo",
        "0.3.0",
        json!({"inference.voice": "^1.0.0", "model.parakeet-tdt-v2-en": "^1.0.0"}),
    );
    let na = w.ext(
        "nano",
        "0.3.0",
        json!({"inference.voice": "^1.0.0", "model.nemotron": "^1.0.0"}),
    );
    w.publish(vec![p, n, v, e, na], vec![], w.now);
}

async fn install(
    w: &World,
    i: &Installer,
    t: &Dirs,
    op: Op,
) -> Result<qrtz_modules::install::Applied, InstallError> {
    i.refresh(t, &RefreshConsent::user_asked(), w.now).await?;
    let plan = i.plan(op)?;
    i.apply(t, &plan, &plan.consent(), w.now, &|_, _| {}).await
}

fn keys_of(i: &Installer) -> Vec<String> {
    i.lock().unwrap().packages.keys().cloned().collect()
}

#[tokio::test]
async fn install_resolves_a_dependency_tree_into_the_store_and_a_text_lock() {
    let mut w = World::new();
    catalogue(&mut w);
    let i = w.installer("c");
    let t = w.hosts();
    let a = install(&w, &i, &t, Op::Install(vec![("echo".into(), None)]))
        .await
        .unwrap();
    assert_eq!(a.changes.len(), 3);
    assert_eq!(
        keys_of(&i),
        [
            "echo@0.3.0",
            "inference.voice@1.0.0",
            "model.parakeet-tdt-v2-en@1.0.0"
        ]
    );
    let lock = std::fs::read_to_string(i.lock_path()).unwrap();
    // Text, one package per key, the workspace's range and resolution, team and requirement recorded.
    assert!(lock.contains("\"echo\": \"^0.3.0\""), "{lock}");
    assert!(lock.contains(&format!("\"teamId\": \"{TEAM}\"")));
    assert!(lock.contains("identifier \\\"ai.qrtz.echo\\\""));
    assert_eq!(
        lock.lines()
            .filter(|l| l.contains("\"designatedRequirement\""))
            .count(),
        2
    );
    // Installed read-only from the store; the helper keeps its executable bit.
    let helper = i.package_path("echo", "0.3.0").join("bin/helper");
    assert_eq!(std::fs::read(&helper).unwrap(), b"echo 0.3.0");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&helper).unwrap().permissions().mode() & 0o777,
            0o500
        );
        assert_eq!(
            std::fs::metadata(i.package_path("echo", "0.3.0"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
    }
    assert!(i.store().has(&qrtz_modules::sha256_hex(&[1u8; 4000]), 4000));
    // Grants: the service declares parakeet, so it may be handed that dir.
    let g = i.model_grants("echo").unwrap();
    assert_eq!(g.len(), 1);
    assert_eq!(
        (g[0].0.as_str(), g[0].1.as_str()),
        ("inference.voice@1.0.0", "model.parakeet-tdt-v2-en@1.0.0")
    );
}

#[tokio::test]
async fn overlapping_ranges_share_one_version_and_disjoint_ones_sit_side_by_side() {
    let mut w = World::new();
    let p = w.model("model.parakeet-tdt-v2-en", "1.0.0", "asr", b"p");
    let n = w.model("model.nemotron", "1.0.0", "asr", b"n");
    let v1 = w.service(
        "inference.voice",
        "1.0.0",
        &["model.parakeet-tdt-v2-en", "model.nemotron"],
    );
    let v12 = w.service(
        "inference.voice",
        "1.2.0",
        &["model.parakeet-tdt-v2-en", "model.nemotron"],
    );
    let v2 = w.service(
        "inference.voice",
        "2.0.0",
        &["model.parakeet-tdt-v2-en", "model.nemotron"],
    );
    // echo wants ~1.0 (so 1.0.x); nano wants ^1.0: 1.0.0 serves both, rather than 1.0.0 + 1.2.0.
    let e = w.ext(
        "echo",
        "0.3.0",
        json!({"inference.voice": "~1.0", "model.parakeet-tdt-v2-en": "^1.0.0"}),
    );
    let na = w.ext(
        "nano",
        "0.3.0",
        json!({"inference.voice": "^1.0.0", "model.nemotron": "^1.0.0"}),
    );
    let c2 = w.ext(
        "ctrl",
        "0.3.0",
        json!({"inference.voice": "^2.0.0", "model.nemotron": "^1.0.0"}),
    );
    w.publish(vec![p, n, v1, v12, v2, e, na, c2], vec![], w.now);
    let i = w.installer("c");
    let t = w.hosts();
    install(&w, &i, &t, Op::Install(vec![("nano".into(), None)]))
        .await
        .unwrap();
    assert!(keys_of(&i).contains(&"inference.voice@1.2.0".to_string()));
    install(&w, &i, &t, Op::Install(vec![("echo".into(), None)]))
        .await
        .unwrap();
    let k = keys_of(&i);
    assert!(k.contains(&"inference.voice@1.0.0".to_string()), "{k:?}");
    assert!(
        !k.contains(&"inference.voice@1.2.0".to_string()),
        "deduped onto 1.0.0: {k:?}"
    );
    // ctrl's ^2 doesn't overlap: a second version, side by side, each requirer pinned to its own.
    install(&w, &i, &t, Op::Install(vec![("ctrl".into(), None)]))
        .await
        .unwrap();
    let lock = i.lock().unwrap();
    assert_eq!(
        lock.get("ctrl", "0.3.0").unwrap().dependencies["inference.voice"],
        "2.0.0"
    );
    assert_eq!(
        lock.get("nano", "0.3.0").unwrap().dependencies["inference.voice"],
        "1.0.0"
    );
    assert!(
        i.package_path("inference.voice", "1.0.0").is_dir()
            && i.package_path("inference.voice", "2.0.0").is_dir()
    );
    // model.nemotron is shared by nano and ctrl: one copy.
    assert_eq!(lock.versions("model.nemotron").len(), 1);
}

#[tokio::test]
async fn switching_an_engine_is_a_dependency_change() {
    let mut w = World::new();
    catalogue(&mut w);
    let i = w.installer("c");
    let t = w.hosts();
    install(&w, &i, &t, Op::Install(vec![("echo".into(), None)]))
        .await
        .unwrap();
    install(
        &w,
        &i,
        &t,
        Op::Engine {
            extension: "echo".into(),
            model: "model.nemotron".into(),
        },
    )
    .await
    .unwrap();
    let lock = i.lock().unwrap();
    let echo = lock.get("echo", "0.3.0").unwrap();
    assert!(echo.dependencies.contains_key("model.nemotron"));
    assert!(!echo.dependencies.contains_key("model.parakeet-tdt-v2-en"));
    assert!(
        lock.versions("model.parakeet-tdt-v2-en").is_empty(),
        "nothing needs parakeet now"
    );
    assert_eq!(
        lock.workspace.overrides["echo"]["model.parakeet-tdt-v2-en"],
        "model.nemotron@^1.0.0"
    );
    // And back.
    install(
        &w,
        &i,
        &t,
        Op::Engine {
            extension: "echo".into(),
            model: "model.parakeet-tdt-v2-en".into(),
        },
    )
    .await
    .unwrap();
    assert!(i.lock().unwrap().workspace.overrides.is_empty());
}

#[tokio::test]
async fn a_model_no_service_can_read_is_refused() {
    // ADR 0212: the publisher runs the same read check as the installer, so it never signs such an index (the
    // installer's refusal of one that was signed anyway is tests/extension_reads.rs on the live index 2 snapshot).
    let mut w = World::new();
    let p = w.model("model.parakeet-tdt-v2-en", "1.0.0", "asr", b"p");
    let v = w.service("inference.voice", "1.0.0", &[]);
    let e = w.ext(
        "echo",
        "0.3.0",
        json!({"inference.voice": "^1.0.0", "model.parakeet-tdt-v2-en": "^1.0.0"}),
    );
    let spec = qrtz_publish::Spec {
        team_id: TEAM.into(),
        packages: [p, v, e]
            .into_iter()
            .map(qrtz_publish::SpecPackage::fake_notarized)
            .collect(),
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    };
    let err = qrtz_publish::build(&w.out, &spec, &w.keys, w.now).unwrap_err();
    assert!(
        err.contains("nothing it runs can read model.parakeet-tdt-v2-en"),
        "{err}"
    );
    let i = w.installer("c");
    assert!(i.lock().unwrap().packages.is_empty());
}

#[tokio::test]
async fn remove_then_prune_drops_what_nothing_references_and_keeps_shared_blobs() {
    let mut w = World::new();
    catalogue(&mut w);
    let i = w.installer("c");
    let t = w.hosts();
    install(
        &w,
        &i,
        &t,
        Op::Install(vec![("echo".into(), None), ("nano".into(), None)]),
    )
    .await
    .unwrap();
    let plan = i.plan(Op::Remove(vec!["echo".into()])).unwrap();
    assert_eq!(plan.download_bytes, 0);
    assert!(plan.changes.contains(&Change::Remove {
        name: "model.parakeet-tdt-v2-en".into(),
        version: "1.0.0".into()
    }));
    i.apply(&t, &plan, &plan.consent(), w.now, &|_, _| {})
        .await
        .unwrap();
    assert_eq!(
        keys_of(&i),
        ["inference.voice@1.0.0", "model.nemotron@1.0.0", "nano@0.3.0"]
    );
    // The dirs stay until prune (a running helper keeps its files).
    assert!(i.package_path("echo", "0.3.0").is_dir());
    let p = i.prune().unwrap();
    assert_eq!(p.dirs.len(), 2, "{p:?}");
    assert!(!i.package_path("echo", "0.3.0").exists());
    assert!(
        !i.store().has(&qrtz_modules::sha256_hex(&[1u8; 4000]), 4000),
        "parakeet's weights are gone"
    );
    assert!(
        i.store().has(&qrtz_modules::sha256_hex(&[2u8; 5000]), 5000),
        "nemotron's stay"
    );
    // Prune twice: nothing more.
    assert_eq!(i.prune().unwrap(), Default::default());
}

#[tokio::test]
async fn locked_versions_stay_until_update() {
    let mut w = World::new();
    catalogue(&mut w);
    let i = w.installer("c");
    let t = w.hosts();
    install(&w, &i, &t, Op::Install(vec![("echo".into(), None)]))
        .await
        .unwrap();
    let v11 = w.service(
        "inference.voice",
        "1.1.0",
        &["model.parakeet-tdt-v2-en", "model.nemotron"],
    );
    w.publish(vec![v11], vec![], w.now + 1);
    // Another install keeps echo's 1.0.0 (stable, like bun) …
    install(&w, &i, &t, Op::Install(vec![("nano".into(), None)]))
        .await
        .unwrap();
    assert!(keys_of(&i).contains(&"inference.voice@1.0.0".to_string()));
    // … until update.
    let a = install(
        &w,
        &i,
        &t,
        Op::Update {
            names: vec![],
            latest: false,
        },
    )
    .await
    .unwrap();
    assert!(a.changes.contains(&Change::Add {
        name: "inference.voice".into(),
        version: "1.1.0".into()
    }));
    assert!(!keys_of(&i).contains(&"inference.voice@1.0.0".to_string()));
}

#[tokio::test]
async fn offline_reinstall_uses_the_lock_and_the_store_only() {
    let mut w = World::new();
    catalogue(&mut w);
    let i = w.installer("c");
    let t = w.hosts();
    install(&w, &i, &t, Op::Install(vec![("echo".into(), None)]))
        .await
        .unwrap();
    // A package dir goes missing; offline (no hosts at all), the frozen plan relinks it from the store.
    qrtz_modules::fsutil::remove_tree(&i.package_path("echo", "0.3.0")).unwrap();
    let none = Dirs::new(vec![]);
    let plan = i.plan(Op::Frozen).unwrap();
    assert_eq!(plan.download_bytes, 0);
    assert!(!plan.online);
    i.apply(&none, &plan, &plan.consent(), w.now, &|_, _| {})
        .await
        .unwrap();
    assert!(i.package_path("echo", "0.3.0").join("bin/helper").exists());
    // Resolving something new offline is impossible without an index.
    let fresh = w.installer("offline");
    assert!(matches!(
        fresh.plan(Op::Install(vec![("echo".into(), None)])),
        Err(InstallError::NoIndex)
    ));
}

#[tokio::test]
async fn blobs_come_in_parallel_ranges_resume_and_fall_back_to_signed_mirrors() {
    let mut w = World::new();
    let big: Vec<u8> = (0..20 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let p = w.model("model.parakeet-tdt-v2-en", "1.0.0", "asr", &big);
    let v = w.service("inference.voice", "1.0.0", &["model.parakeet-tdt-v2-en"]);
    let e = w.ext(
        "echo",
        "0.3.0",
        json!({"inference.voice": "^1.0.0", "model.parakeet-tdt-v2-en": "^1.0.0"}),
    );
    w.publish(vec![p, v, e], vec![], w.now);
    let i = w.installer("c");
    let t = w.hosts();
    let sha = qrtz_modules::sha256_hex(&big);
    // One range of the big blob fails once: the first attempt fails, the retry fetches only what's left.
    t.fail_once
        .lock()
        .unwrap()
        .insert(format!("{}@{}", &sha[..8], 8 * 1024 * 1024));
    i.refresh(&t, &RefreshConsent::user_asked(), w.now).await.unwrap();
    let plan = i.plan(Op::Install(vec![("echo".into(), None)])).unwrap();
    assert!(i
        .apply(&t, &plan, &plan.consent(), w.now, &|_, _| {})
        .await
        .is_err());
    let before = t.ranges.lock().unwrap().iter().filter(|(s, _)| *s == sha).count();
    assert_eq!(before, 2, "ranges 0 and 2 landed");
    i.apply(&t, &plan, &plan.consent(), w.now, &|_, _| {})
        .await
        .unwrap();
    let after: Vec<u64> = t
        .ranges
        .lock()
        .unwrap()
        .iter()
        .filter(|(s, _)| *s == sha)
        .map(|(_, o)| *o)
        .collect();
    assert_eq!(
        after.len(),
        3,
        "only the missing range was fetched again: {after:?}"
    );
    // A model file: only its Hugging Face URL in the signed tree serves it, checked by the same sha256.
    let mut w2 = World::new();
    let d = w2.model("model.parakeet-tdt-v2-en", "1.0.0", "asr", b"mirrored");
    w2.publish(vec![d], vec![], w2.now);
    let wsha = qrtz_modules::sha256_hex(b"mirrored");
    // ADR 0171: model bytes are never in the published output (modules.qrtz.ai doesn't host them).
    assert!(!w2.out.join(layout::blob(&wsha)).exists());
    // The fetcher itself, on a blob whose host copy is gone and whose signed tree lists a mirror URL.
    let store = qrtz_modules::store::Store::new(w2.base.join("store"));
    let mirror_dir = w2.base.join("hf");
    write(
        &mirror_dir.join("org/r/resolve/abc0000000000000000000000000000000000000/w.bin"),
        b"mirrored",
        false,
    );
    let mut t2 = Dirs::new(vec![w2.out.clone()]);
    t2.urls
        .insert("https://huggingface.co/".into(), mirror_dir.clone());
    let b = qrtz_modules::index::BlobRef {
        sha256: wsha.clone(),
        size: 8,
        mirrors: vec![
            "https://huggingface.co/org/r/resolve/abc0000000000000000000000000000000000000/w.bin".into(),
        ],
    };
    qrtz_modules::fetch::fetch_blobs(&t2, &store, std::slice::from_ref(&b), &|_, _| {})
        .await
        .unwrap();
    assert!(store.has(&wsha, 8));
    // A mirror with other bytes is refused (nothing enters the store).
    store.remove(&wsha).unwrap();
    write(
        &mirror_dir.join("org/r/resolve/abc0000000000000000000000000000000000000/w.bin"),
        b"tampered",
        false,
    );
    assert!(qrtz_modules::fetch::fetch_blobs(&t2, &store, &[b], &|_, _| {})
        .await
        .is_err());
    assert!(!store.has(&wsha, 8));
}

#[tokio::test]
async fn revoked_versions_are_never_picked_and_a_fresh_list_disables_installed_ones() {
    let mut w = World::new();
    catalogue(&mut w);
    let i = w.installer("c");
    let t = w.hosts();
    install(&w, &i, &t, Op::Install(vec![("echo".into(), None)]))
        .await
        .unwrap();
    // inference.voice 1.0.0 is pulled and 1.0.1 published.
    let v101 = w.service(
        "inference.voice",
        "1.0.1",
        &["model.parakeet-tdt-v2-en", "model.nemotron"],
    );
    let rv = Revoked {
        package: "inference.voice".into(),
        versions: vec!["1.0.0".into()],
        reason: "crashes".into(),
    };
    w.publish(vec![v101], vec![rv], w.now + 1);
    let r = i
        .refresh(&t, &RefreshConsent::user_asked(), w.now + 2)
        .await
        .unwrap();
    assert_eq!(
        r.notices.len(),
        2,
        "the service and the extension that depends on it: {:?}",
        r.notices
    );
    let lock = i.lock().unwrap();
    assert!(lock.get("echo", "0.3.0").unwrap().disabled.is_some());
    // An update moves off the revoked version; it is never picked again.
    let plan = i
        .plan(Op::Update {
            names: vec![],
            latest: false,
        })
        .unwrap();
    i.apply(&t, &plan, &plan.consent(), w.now + 2, &|_, _| {})
        .await
        .unwrap();
    assert_eq!(
        i.lock().unwrap().get("echo", "0.3.0").unwrap().dependencies["inference.voice"],
        "1.0.1"
    );
}

#[tokio::test]
async fn an_online_install_needs_a_fresh_list_and_a_matching_consent() {
    let mut w = World::new();
    catalogue(&mut w);
    let i = w.installer("c");
    let t = w.hosts();
    i.refresh(&t, &RefreshConsent::user_asked(), w.now).await.unwrap();
    let plan = i.plan(Op::Install(vec![("echo".into(), None)])).unwrap();
    // A consent for another plan.
    let mut c = plan.consent();
    c.plan_sha256 = "0".repeat(64);
    assert_eq!(
        i.apply(&t, &plan, &c, w.now, &|_, _| {}).await.unwrap_err(),
        InstallError::ConsentMismatch
    );
    // Eight days later the stored list has expired: an online install is refused (rule 1, no override).
    let late = w.now + 8 * DAY_MS;
    assert_eq!(
        i.apply(&t, &plan, &plan.consent(), late, &|_, _| {})
            .await
            .unwrap_err(),
        InstallError::Refused(Refusal::RevocationListExpired)
    );
    // A freeze attack (a host serving the old timestamp forever) can't refresh it either.
    assert!(matches!(
        i.refresh(&t, &RefreshConsent::user_asked(), late).await,
        Err(InstallError::Index(
            qrtz_modules::index::IndexError::Expired { .. }
        ))
    ));
    // A plan made before the lock changed is stale.
    i.apply(&t, &plan, &plan.consent(), w.now, &|_, _| {})
        .await
        .unwrap();
    assert_eq!(
        i.apply(&t, &plan, &plan.consent(), w.now, &|_, _| {})
            .await
            .unwrap_err(),
        InstallError::StalePlan
    );
}

#[tokio::test]
async fn app_pinned_models_install_offline_and_merge_with_the_indexed_package() {
    let mut w = World::new();
    // The pinned set, exactly as qrtz_modules::models makes it; its file is served only by a "Hugging Face" dir.
    let src = qrtz_modules::models::ModelSource {
        package: "model.parakeet-tdt-v2-en".into(),
        version: "1.0.0".into(),
        task: "asr".into(),
        repo: "org/parakeet".into(),
        revision: "ee09c569f73759e6d44c9bd16766f477b2b36d39".into(),
        license: String::new(),
        files: vec![qrtz_modules::models::ModelFile {
            remote: "Encoder.mlmodelc/weights.bin".into(),
            path: "Encoder.mlmodelc/weights.bin".into(),
            size: 6,
            sha256: qrtz_modules::sha256_hex(b"weight"),
        }],
    };
    let hf = w.base.join("hf");
    write(
        &hf.join(
            "org/parakeet/resolve/ee09c569f73759e6d44c9bd16766f477b2b36d39/Encoder.mlmodelc/weights.bin",
        ),
        b"weight",
        false,
    );
    let i = w.installer("c");
    let mut t = Dirs::new(vec![]);
    t.urls.insert("https://huggingface.co/".into(), hf);
    // No index, no hosts, no revocation list: a pinned model (data, hash-pinned in the app) still installs.
    let plan = i.plan(Op::Pinned(vec![src.pinned(TEAM)])).unwrap();
    assert_eq!(plan.download_bytes, 6);
    i.apply(&t, &plan, &plan.consent(), w.now, &|_, _| {})
        .await
        .unwrap();
    let lock = i.lock().unwrap();
    assert_eq!(lock.workspace.dependencies["model.parakeet-tdt-v2-en"], "=1.0.0");
    assert_eq!(
        lock.get("model.parakeet-tdt-v2-en", "1.0.0").unwrap().source,
        qrtz_modules::qrtz_lock::Source::Pinned
    );
    // Code can't be pinned this way.
    let mut code = src.pinned(TEAM);
    code.name = "inference.voice".into();
    assert!(matches!(
        i.plan(Op::Pinned(vec![code])),
        Err(InstallError::PinnedCode(_)) | Err(InstallError::ManifestMismatch { .. })
    ));
    // Later the index carries the same package (same tree): echo's dependency dedupes onto the pinned install.
    let pub_dir = w.dir();
    for f in &src.files {
        write(&pub_dir.join(&f.path), b"weight", false);
    }
    std::fs::write(w.base.join("pins.json"), serde_json::to_vec(&json!({"version": "1.0.0", "sets": [{
        "package": src.package, "task": "asr", "repo": src.repo, "revision": src.revision,
        "files": [{"remote": "Encoder.mlmodelc/weights.bin", "path": "Encoder.mlmodelc/weights.bin", "size": 6, "sha256": src.files[0].sha256}]}]})).unwrap()).unwrap();
    let v = w.service("inference.voice", "1.0.0", &["model.parakeet-tdt-v2-en"]);
    let e = w.ext(
        "echo",
        "0.3.0",
        json!({"inference.voice": "^1.0.0", "model.parakeet-tdt-v2-en": "^1.0.0"}),
    );
    let spec = qrtz_publish::Spec {
        team_id: TEAM.into(),
        packages: vec![v, e]
            .into_iter()
            .map(qrtz_publish::SpecPackage::fake_notarized)
            .collect(),
        pinned_models: vec![w.base.join("pins.json")],
        models: vec![qrtz_publish::SpecModel {
            name: src.package.clone(),
            dir: pub_dir,
        }],
        revoked: vec![],
    };
    qrtz_publish::build(&w.out, &spec, &w.keys, w.now).unwrap();
    let t = w.hosts();
    let before = i.store().list().len();
    install(&w, &i, &t, Op::Install(vec![("echo".into(), None)]))
        .await
        .unwrap();
    let lock = i.lock().unwrap();
    assert_eq!(lock.versions("model.parakeet-tdt-v2-en").len(), 1, "one copy");
    assert_eq!(
        lock.get("echo", "0.3.0").unwrap().dependencies["model.parakeet-tdt-v2-en"],
        "1.0.0"
    );
    assert_eq!(
        i.store().list().len(),
        before + 6,
        "only echo's and the service's tree, qrtz.json and code were fetched; the model was not"
    );
}

#[tokio::test]
async fn a_sideload_installs_exactly_the_files_set_and_a_revoked_one_is_refused() {
    let mut w = World::new();
    catalogue(&mut w);
    let pkg = w.base.join("echo.qrtzpkg");
    let set = qrtz_publish::pack(&w.out, "echo", "0.3.0", &pkg).unwrap();
    assert_eq!(
        set,
        [
            "echo@0.3.0",
            "inference.voice@1.0.0",
            "model.parakeet-tdt-v2-en@1.0.0"
        ]
    );
    let i = w.installer("c");
    let late = w.now + 30 * DAY_MS;
    let prompt = match i
        .sideload(
            &pkg,
            qrtz_modules::sideload::SideloadOrigin::UserPickedFile,
            None,
            late,
        )
        .await
    {
        Err(InstallError::Refused(Refusal::NeedsConfirmation(p))) => p,
        other => panic!("{other:?}"),
    };
    assert!(prompt.revocation_expired);
    let yes = qrtz_modules::sideload::Confirmation {
        package_sha256: prompt.package_sha256,
        revocation_issued_ms: prompt.revocation_issued_ms,
    };
    let a = i
        .sideload(
            &pkg,
            qrtz_modules::sideload::SideloadOrigin::UserPickedFile,
            Some(&yes),
            late,
        )
        .await
        .unwrap();
    assert!(a.stale_revocation);
    assert_eq!(keys_of(&i).len(), 3);
    assert!(i
        .lock()
        .unwrap()
        .packages
        .values()
        .all(|l| l.stale_revocation && l.kind != PackageKind::Shell));
    // A watched-folder file is refused outright.
    assert_eq!(
        i.sideload(
            &pkg,
            qrtz_modules::sideload::SideloadOrigin::WatchedFolder,
            Some(&yes),
            late
        )
        .await
        .unwrap_err(),
        InstallError::Refused(Refusal::NotUserInitiated)
    );
}

#[test]
fn a_package_with_an_install_script_cant_even_be_published() {
    let mut w = World::new();
    let d = w.dir();
    write(
        &d.join("qrtz.json"),
        br#"{"name":"model.x","version":"1.0.0","kind":"model","model":{"task":"asr"},"scripts":{"postinstall":"sh x"}}"#,
        false,
    );
    let spec = qrtz_publish::Spec {
        team_id: TEAM.into(),
        packages: vec![qrtz_publish::SpecPackage::fake_notarized(d)],
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    };
    let e = qrtz_publish::build(&w.out, &spec, &w.keys, w.now).unwrap_err();
    assert!(e.contains("no install scripts"), "{e}");
}

#[test]
fn the_legacy_lockfile_migrates_to_requests() {
    let d = tempfile::tempdir().unwrap();
    let old = qrtz_modules::lock::Lockfile::parse(qrtz_modules::bundled::LOCK.as_bytes()).unwrap();
    let reqs = qrtz_modules::qrtz_lock::migrate_legacy(&old);
    // Every bundled extension (echo, nano, ctrl and, ADR 0159, snap) becomes a caret request on the version its
    // module manifest carries, so a release bump doesn't need this test edited.
    let expected: BTreeMap<String, String> = qrtz_modules::bundled::manifests()
        .into_iter()
        .map(|(id, src)| {
            let m = qrtz_modules::ModuleManifest::parse(src.as_bytes()).unwrap();
            (id.to_string(), format!("^{}", m.version))
        })
        .collect();
    assert_eq!(
        expected.keys().map(String::as_str).collect::<Vec<_>>(),
        ["ctrl", "echo", "nano", "snap"]
    );
    assert_eq!(reqs, expected);
    // snap's legacy entry is pinned exactly: version, manifest hash, team and designated requirement, and the name
    // is an extension (so the migration keeps it; a model or service entry would be dropped).
    let snap = &old.modules["snap"];
    let snap_manifest = qrtz_modules::ModuleManifest::parse(qrtz_modules::bundled::SNAP.as_bytes()).unwrap();
    assert_eq!(snap.version, snap_manifest.version);
    assert_eq!(
        snap.manifest_sha256,
        qrtz_modules::sha256_hex(qrtz_modules::bundled::SNAP.as_bytes())
    );
    assert_eq!(snap.code, snap_manifest.code);
    assert_eq!(snap.code.bundle_id, "ai.qrtz.snap");
    assert_eq!(
        snap.code.team_id,
        qrtz_modules::manifest::RELEASE_TEAM_ID,
        "snap pins Jared's Developer ID team"
    );
    assert_eq!(
        snap.code.designated_requirement,
        qrtz_modules::manifest::designated_requirement("ai.qrtz.snap", &snap.code.team_id)
    );
    assert!(snap.dependencies.is_empty());
    assert_eq!(
        qrtz_modules::qrtz_json::kind_of("snap").unwrap(),
        PackageKind::Extension
    );
    std::fs::create_dir_all(d.path().join("modules")).unwrap();
    std::fs::write(
        d.path().join("modules/modules.lock.json"),
        qrtz_modules::bundled::LOCK,
    )
    .unwrap();
    let i = Installer::new(d.path(), None, Box::new(NoAppleChecks));
    assert_eq!(i.migrate_legacy().unwrap(), reqs);
    assert!(d.path().join("modules/modules.lock.json.migrated").exists());
    assert!(i.migrate_legacy().unwrap().is_empty());
    assert_eq!(
        qrtz_modules::qrtz_lock::legacy_name("model:parakeet-tdt-v2"),
        "model.parakeet-tdt-v2-en"
    );
}

/// ADR 0212: an extension's process gets read-only store paths for exactly the models it declares in
/// `extension.reads`, derived from `qrtz.lock`; another unit's model, or an undeclared one, is refused.
#[tokio::test]
async fn a_model_an_extension_reads_needs_a_fresh_list_and_loads_read_only_from_the_store() {
    let mut w = World::new();
    let l = w.model("model.laya", "1.0.0", "decision", b"laya");
    let n = w.model("model.nemotron", "1.0.0", "asr", b"nemo");
    let reader = |w: &mut World, name: &str, model: &str| {
        let d = w.dir();
        write(
            &d.join("qrtz.json"),
            &serde_json::to_vec(&json!({"name": name, "version": "0.3.0", "kind": "extension",
                "dependencies": {model: "^1.0.0"},
                "extension": {"entry": "bin/helper", "protocol": 1, "minApp": "0.3.0", "reads": [model]}}))
            .unwrap(),
            false,
        );
        write(&d.join("bin/helper"), name.as_bytes(), true);
        d
    };
    let c = reader(&mut w, "ctrl", "model.laya");
    let nano = reader(&mut w, "nano", "model.nemotron");
    w.publish(vec![l, n, c, nano], vec![], w.now);
    let i = w.installer("c");
    let t = w.hosts();
    assert!(i.extension_read_path("ctrl", "model.laya", true).is_err());
    install(
        &w,
        &i,
        &t,
        Op::Install(vec![("ctrl".into(), None), ("nano".into(), None)]),
    )
    .await
    .unwrap();
    let dir = i.extension_read_path("ctrl", "model.laya", true).unwrap();
    assert_eq!(dir, i.package_path("model.laya", "1.0.0"));
    let mode = std::fs::metadata(dir.join("weights.bin")).unwrap().permissions();
    assert!(mode.readonly());
    // Each process gets its own declared models and nothing else: not another unit's, not an undeclared one.
    assert_eq!(
        i.process_reads("nano").unwrap(),
        [(
            "model.nemotron".to_owned(),
            i.package_path("model.nemotron", "1.0.0")
        )]
    );
    assert_eq!(
        i.process_reads("ctrl").unwrap(),
        [("model.laya".to_owned(), i.package_path("model.laya", "1.0.0"))]
    );
    assert!(i.extension_read_path("nano", "model.laya", false).is_err());
    assert!(i.extension_read_path("ctrl", "model.nemotron", false).is_err());
    assert!(i.extension_read_path("echo", "model.laya", false).is_err());
    assert!(i.process_reads("echo").is_err());
    // A writable file is refused at load.
    let f = dir.join("weights.bin");
    let keep = std::fs::metadata(&f).unwrap().permissions();
    let mut p = keep.clone();
    #[allow(clippy::permissions_set_readonly_false)]
    p.set_readonly(false);
    std::fs::set_permissions(&f, p).unwrap();
    assert!(i.extension_read_path("ctrl", "model.laya", false).is_err());
    assert!(i.process_reads("ctrl").unwrap().is_empty());
    std::fs::set_permissions(&f, keep).unwrap();
    assert!(i.extension_read_path("ctrl", "model.laya", true).is_ok());
}

/// ADR 0212: an extension's index entry carries `models` as its `extension.reads` (each a dependency); the publisher
/// writes it from `extension.reads` and the client re-checks it.
#[tokio::test]
async fn an_extension_index_entry_carries_models_only_as_its_reads_exception() {
    let mut w = World::new();
    let l = w.model("model.laya", "1.0.0", "decision", &[3u8; 3000]);
    let d = w.dir();
    write(
        &d.join("qrtz.json"),
        &serde_json::to_vec(&json!({"name": "ctrl", "version": "0.3.0", "kind": "extension",
            "dependencies": {"model.laya": "^1.0.0"},
            "extension": {"entry": "bin/helper", "protocol": 1, "minApp": "0.3.0", "reads": ["model.laya"]}}))
        .unwrap(),
        false,
    );
    write(&d.join("bin/helper"), b"ctrl", true);
    w.publish(vec![l, d], vec![], w.now);
    let i = w.installer("c");
    i.refresh(&w.hosts(), &RefreshConsent::user_asked(), w.now)
        .await
        .unwrap();
    let (_, v) = i.cached_index().unwrap();
    let e = v.index.entry("ctrl", "0.3.0").unwrap().clone();
    assert_eq!(e.models, ["model.laya"]);
    e.validate("ctrl").unwrap();
    // ADR 0212: any model it depends on may be read (no allowlist); one it doesn't depend on may not.
    let mut x = e.clone();
    x.models = vec!["model.laya".into(), "model.nemotron".into()];
    x.dependencies.insert("model.nemotron".into(), "1.0.0".into());
    x.validate("ctrl").unwrap();
    let mut x = e.clone();
    x.models = vec!["model.nemotron".into()];
    assert!(x.validate("ctrl").is_err());
    let mut x = e;
    x.dependencies.clear();
    assert!(x.validate("ctrl").is_err());
}
