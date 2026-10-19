//! qrtz-publish (ADR 0153, ADR 0157): the release side of the signed package index.
//!
//! The output dir is exactly what goes to the bucket (`rclone sync <out> r2:qrtz-modules` and the B2 mirror):
//!
//! ```text
//! v1/root/<N>.root.json(.sig)      roots, never changed once written
//! v1/timestamp.json(.sig)          re-signed at least weekly (it expires after 7 days; the client allows 8)
//! v1/index/<sha256>.json(.sig)     one per publish, immutable, named by its hash
//! v1/blobs/sha256/<hex>            every file and every package tree, whole, immutable
//! ```
//!
//! Blobs and indexes are never overwritten or deleted; pulling a release is a new index with it in `revoked`.
//! Packages are extensions, services and models, each with a `qrtz.json` (ADR 0157). Model sets pinned in the app
//! (`local_models.json`, `ctrl_laya_model.json`) are published from a dir of their downloaded files, checked
//! against those pins, so the hosted package is the same tree the app already pins.
pub mod keys;

use ed25519_dalek::SigningKey;
use keys::{sign, Keys};
use qrtz_modules::index::{
    canonical, key_id, layout, Index, IndexFile, IndexRef, PackageEntry, PackageKind, PublicKey, Role, Roles,
    Root, Timestamp, Tree, DAY_MS, MANIFEST_FILE, PLATFORM,
};
use qrtz_modules::manifest::{designated_requirement, is_team_id};
use qrtz_modules::models::ModelSource;
use qrtz_modules::qrtz_json::{identifier_for, PackageManifest};
use qrtz_modules::sideload::Revoked;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The timestamp is signed for 7 days, inside the client's 8-day clamp, so a weekly re-sign has a day of slack.
pub const TIMESTAMP_DAYS: u64 = 7;
pub const INDEX_DAYS: u64 = 365;
pub const ROOT_DAYS: u64 = 365;

pub type Result<T> = std::result::Result<T, String>;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn io<E: std::fmt::Display>(p: &Path) -> impl FnOnce(E) -> String + '_ {
    move |e| format!("{}: {e}", p.display())
}

/// Write `bytes` at `out/<rel>`; an existing file must already hold exactly these bytes (immutable paths).
fn put_immutable(out: &Path, rel: &str, bytes: &[u8]) -> Result<()> {
    let p = out.join(rel);
    if let Ok(have) = std::fs::read(&p) {
        return if have == bytes {
            Ok(())
        } else {
            Err(format!(
                "{rel}: exists with other bytes; published paths never change"
            ))
        };
    }
    put(out, rel, bytes)
}

/// Write (or replace) `out/<rel>` through a temp file: only the timestamp is replaced.
fn put(out: &Path, rel: &str, bytes: &[u8]) -> Result<()> {
    let p = out.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).map_err(io(&p))?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io(&tmp))?;
    std::fs::rename(&tmp, &p).map_err(io(&p))
}

fn role(keys: &[ed25519_dalek::VerifyingKey]) -> Vec<String> {
    let mut v: Vec<String> = keys.iter().map(|k| key_id(&k.to_bytes())).collect();
    v.sort();
    v
}

fn public(k: &ed25519_dalek::VerifyingKey) -> (String, PublicKey) {
    (
        key_id(&k.to_bytes()),
        PublicKey {
            scheme: "ed25519".into(),
            public: hex::encode(k.to_bytes()),
        },
    )
}

/// The highest root version in `out`, if any.
pub fn highest_root(out: &Path) -> Option<u64> {
    let d = std::fs::read_dir(out.join("v1/root")).ok()?;
    d.flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".root.json")?
                .parse::<u64>()
                .ok()
        })
        .max()
}

fn read_root(out: &Path, n: u64) -> Result<Root> {
    let p = out.join(layout::root(n));
    Root::parse(&std::fs::read(&p).map_err(io(&p))?).map_err(|e| e.to_string())
}

/// `root`: write root `N+1` (or 1) naming `keys`' root, timestamp and index public keys, signed by every root
/// private key in `keys` and in `previous` (root N's keys). Returns the new version and its sha256 (the pin, for
/// root 1).
pub fn write_root(
    out: &Path,
    keys: &Keys,
    previous: Option<&Keys>,
    threshold: u32,
    now: u64,
) -> Result<(u64, String)> {
    let n = highest_root(out).map_or(1, |n| n + 1);
    let root_keys = keys.root_keys();
    if root_keys.is_empty() {
        return Err("no root keys (root-*.key or root-*.pub)".into());
    }
    let ts = keys
        .timestamp_public
        .ok_or("no timestamp key (timestamp.key or timestamp.pub)")?;
    let ix = keys.index_public.ok_or("no index key (index.key or index.pub)")?;
    let mut all = root_keys.clone();
    all.extend([ts, ix]);
    let r = Root {
        kind: "root".into(),
        version: n,
        issued_ms: now,
        expires_ms: now + ROOT_DAYS * DAY_MS,
        keys: all.iter().map(public).collect(),
        roles: Roles {
            root: Role {
                keyids: role(&root_keys),
                threshold,
            },
            timestamp: Role {
                keyids: role(&[ts]),
                threshold: 1,
            },
            index: Role {
                keyids: role(&[ix]),
                threshold: 1,
            },
        },
    };
    r.validate().map_err(|e| e.to_string())?;
    let bytes = canonical(&r);
    let mut signers: Vec<&SigningKey> = keys.root.iter().collect();
    if let Some(p) = previous {
        signers.extend(p.root.iter());
    }
    let sig = sign(&bytes, &signers);
    let sigs = qrtz_modules::index::Signatures::parse(&sig).map_err(|e| e.to_string())?;
    r.verify_role("new root", &r.roles.root, &bytes, &sigs)
        .map_err(|e| format!("{e}: sign with the new root keys"))?;
    if n > 1 {
        let old = read_root(out, n - 1)?;
        old.verify_role("old root", &old.roles.root, &bytes, &sigs)
            .map_err(|e| format!("{e}: sign with root {}'s keys too (--prev-keys)", n - 1))?;
    }
    put_immutable(out, &layout::root(n), &bytes)?;
    put_immutable(out, &layout::sig(&layout::root(n)), &sig)?;
    Ok((n, qrtz_modules::sha256_hex(&bytes)))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpecPackage {
    /// The built package dir. It holds the package's `qrtz.json` (name, version, kind, dependency ranges) and its
    /// files: an extension's helper executable, a service's `.xpc`, a model's weights.
    pub dir: PathBuf,
    #[serde(default)]
    pub min_os: Option<String>,
    /// Extension and service (ADR 0170): Apple's Accepted notarization of the package's code, from `notarytool`
    /// (scripts/modules/publish.sh writes it). Code without one is never published.
    #[serde(default)]
    pub notarization: Option<qrtz_modules::index::Notarization>,
}

impl SpecPackage {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            min_os: None,
            notarization: None,
        }
    }

    /// Dry runs and tests only (keys made at runtime, nothing uploaded): a well-formed Accepted record for the
    /// package's code as it is on disk, with a made-up submission id and cdhash. A real publish never calls this:
    /// publish.sh records what notarytool answered.
    pub fn fake_notarized(dir: PathBuf) -> Self {
        let notarization = fake_notarization(&dir);
        Self {
            dir,
            min_os: None,
            notarization,
        }
    }
}

/// See [`SpecPackage::fake_notarized`]. `None` for models (and anything unreadable).
pub fn fake_notarization(dir: &Path) -> Option<qrtz_modules::index::Notarization> {
    let m = PackageManifest::parse(&std::fs::read(dir.join(MANIFEST_FILE)).ok()?).ok()?;
    let code = match (&m.extension, &m.service) {
        (Some(e), _) => dir.join(&e.entry),
        (_, Some(s)) => {
            let macos = dir.join(&s.bundle).join("Contents/MacOS");
            std::fs::read_dir(&macos).ok()?.flatten().next()?.path()
        }
        _ => return None,
    };
    let sha = qrtz_modules::sha256_hex(&std::fs::read(code).ok()?);
    Some(qrtz_modules::index::Notarization {
        submission_id: "00000000-0000-4000-8000-000000000000".into(),
        status: qrtz_modules::index::NOTARIZATION_ACCEPTED.into(),
        cdhash: sha[..40].to_owned(),
        code_sha256: sha,
    })
}

/// A model set pinned in the app, published from a dir holding its downloaded files.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpecModel {
    /// `model.parakeet-tdt-v2-en`.
    pub name: String,
    /// The downloaded files, at their package paths.
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spec {
    /// The Developer ID team (also the publisher team of models).
    pub team_id: String,
    #[serde(default)]
    pub packages: Vec<SpecPackage>,
    /// Pinned model sets (`local_models.json`-format files, relative to the spec) and the dirs of their files.
    #[serde(default)]
    pub pinned_models: Vec<PathBuf>,
    #[serde(default)]
    pub models: Vec<SpecModel>,
    /// Added to every list published before (the set never shrinks).
    #[serde(default)]
    pub revoked: Vec<Revoked>,
}

impl Spec {
    pub fn load(p: &Path) -> Result<Self> {
        let s: Spec = serde_json::from_slice(&std::fs::read(p).map_err(io(p))?).map_err(io(p))?;
        // Paths are relative to the spec file.
        let base = p.parent().unwrap_or(Path::new("."));
        let abs = |d: PathBuf| if d.is_relative() { base.join(d) } else { d };
        Ok(Spec {
            packages: s
                .packages
                .into_iter()
                .map(|x| SpecPackage { dir: abs(x.dir), ..x })
                .collect(),
            pinned_models: s.pinned_models.into_iter().map(abs).collect(),
            models: s
                .models
                .into_iter()
                .map(|m| SpecModel { dir: abs(m.dir), ..m })
                .collect(),
            ..s
        })
    }
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf, bool)>) -> Result<()> {
    let mut es: Vec<_> = std::fs::read_dir(dir).map_err(io(dir))?.flatten().collect();
    es.sort_by_key(|e| e.file_name());
    for e in es {
        let p = e.path();
        let ft = e.file_type().map_err(io(&p))?;
        let rel = p
            .strip_prefix(base)
            .expect("inside")
            .to_string_lossy()
            .replace('\\', "/");
        if ft.is_symlink() {
            return Err(format!("{rel}: symlinks aren't published (flatten the bundle)"));
        } else if ft.is_dir() {
            walk(&p, base, out)?;
        } else if ft.is_file() {
            if rel == ".DS_Store" || rel.ends_with("/.DS_Store") {
                continue;
            }
            #[cfg(unix)]
            let exec = {
                use std::os::unix::fs::PermissionsExt;
                e.metadata().map_err(io(&p))?.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let exec = false;
            out.push((rel, p, exec));
        }
    }
    Ok(())
}

/// ADR 0171: where the publisher keeps its copies of model files, beside `out` and never inside it (so nothing that
/// uploads `out` can put model bytes on modules.qrtz.ai): `<out>.model-blobs/v1/blobs/sha256/<hex>`. Only `pack`
/// reads it, because a `.qrtzpkg` is an offline install and carries everything.
pub fn model_blob_dir(out: &Path) -> PathBuf {
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    out.with_file_name(format!("{name}.model-blobs"))
}

/// Hash a file and copy it into `out` as a blob (whole: a big file is served as one object and fetched in ranges).
fn blob_file(out: &Path, src: &Path) -> Result<(u64, String)> {
    let (size, sha) = qrtz_modules::fsutil::sha256_file(src).map_err(io(src))?;
    let dest = out.join(layout::blob(&sha));
    if !dest.exists() {
        std::fs::create_dir_all(dest.parent().expect("parent")).map_err(io(&dest))?;
        let tmp = dest.with_extension("incoming");
        std::fs::copy(src, &tmp).map_err(io(src))?;
        let (n, h) = qrtz_modules::fsutil::sha256_file(&tmp).map_err(io(&tmp))?;
        if n != size || h != sha {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: changed while publishing", src.display()));
        }
        std::fs::rename(&tmp, &dest).map_err(io(&dest))?;
    }
    Ok((size, sha))
}

/// The index entry for a tree and its manifest (identity from the team), with the tree written as a blob.
fn entry_for(
    out: &Path,
    team: &str,
    m: &PackageManifest,
    tree: Tree,
    min_os: Option<String>,
    notarization: Option<qrtz_modules::index::Notarization>,
) -> Result<PackageEntry> {
    let msha = tree
        .file(MANIFEST_FILE)
        .map(|f| f.sha256.clone())
        .ok_or("a package needs its qrtz.json")?;
    let identifier = identifier_for(&m.name, m.kind);
    let designated = identifier.as_deref().map(|i| designated_requirement(i, team));
    let models = match m.kind {
        PackageKind::Service => m.service_models().to_vec(),
        PackageKind::Extension => m.extension.as_ref().map(|e| e.reads.clone()).unwrap_or_default(),
        _ => vec![],
    };
    let tb = tree.bytes();
    put_immutable(out, &layout::blob(&tree.sha256()), &tb)?;
    let e = PackageEntry {
        kind: m.kind,
        team_id: team.into(),
        sha256: tree.sha256(),
        size: tree.size(),
        files: tree.files,
        manifest_sha256: msha,
        dependencies: m.dependencies.clone(),
        identifier: if m.kind.has_code() { identifier } else { None },
        designated_requirement: if m.kind.has_code() { designated } else { None },
        task: m.task().map(str::to_owned),
        models,
        platform: PLATFORM.into(),
        min_os,
        notarization: if m.kind.has_code() { notarization } else { None },
    };
    e.validate(&m.name)
        .map_err(|why| format!("{}@{}: {why}", m.name, m.version))?;
    Ok(e)
}

/// One package from its built dir: blobs into `out`, (name, version, entry) back.
pub fn package_entry(out: &Path, team: &str, p: &SpecPackage) -> Result<(String, String, PackageEntry)> {
    let mp = p.dir.join(MANIFEST_FILE);
    let bytes =
        std::fs::read(&mp).map_err(|_| format!("{}: a package dir needs qrtz.json", p.dir.display()))?;
    let m = PackageManifest::parse(&bytes).map_err(|e| format!("{}: {e}", mp.display()))?;
    let mut files = vec![];
    walk(&p.dir, &p.dir, &mut files)?;
    // ADR 0171: a model's files are hashed here but never published to modules.qrtz.ai: each one is listed with its
    // Hugging Face URL at the commit its qrtz.json pins (`model.upstream` + `model.revision`), which the installer
    // fetches and checks against this hash.
    let hf = match &m.model {
        Some(mm) if m.kind == PackageKind::Model => {
            let url = format!(
                "{}/{}/resolve/{}/",
                qrtz_modules::models::HUGGING_FACE,
                mm.upstream,
                mm.revision
            );
            if !qrtz_modules::index::is_pinned_hugging_face_url(&format!("{url}x")) {
                return Err(format!(
                    "{}@{}: model.upstream must be <owner>/<repo> on Hugging Face and model.revision a 40-hex \
                     commit (not a branch) (ADR 0171)",
                    m.name, m.version
                ));
            }
            Some(url)
        }
        _ => None,
    };
    let mut entries = vec![];
    for (rel, src, exec) in &files {
        let weights = hf.as_ref().filter(|_| rel != MANIFEST_FILE);
        let (size, sha256) = match weights {
            Some(_) => blob_file(&model_blob_dir(out), src)?,
            None => blob_file(out, src)?,
        };
        entries.push(IndexFile {
            path: rel.clone(),
            size,
            sha256,
            executable: *exec,
            mirrors: weights.map(|u| vec![format!("{u}{rel}")]).unwrap_or_default(),
        });
    }
    if m.kind.has_code() && p.notarization.is_none() {
        return Err(format!(
            "{}@{}: code is published only notarized (ADR 0170): run scripts/modules/publish.sh, which records \
             notarytool's Accepted result",
            m.name, m.version
        ));
    }
    let e = entry_for(
        out,
        team,
        &m,
        Tree::new(entries),
        p.min_os.clone(),
        p.notarization.clone(),
    )?;
    Ok((m.name, m.version, e))
}

/// A pinned model set from the dir of its downloaded files: every file must match its pin, and the tree (with the
/// pinned Hugging Face URLs) is exactly the one the app pins. ADR 0171: the files are only hashed, never written to
/// `out`: modules.qrtz.ai doesn't host model bytes, the client fetches them from Hugging Face. Only the small
/// qrtz.json and the tree are published.
pub fn model_entry(out: &Path, team: &str, src: &ModelSource, dir: &Path) -> Result<PackageEntry> {
    for f in &src.files {
        let p = dir.join(&f.path);
        let (n, h) = blob_file(&model_blob_dir(out), &p)?;
        if n != f.size || h != f.sha256 {
            return Err(format!(
                "{}: doesn't match the pin for {}",
                p.display(),
                src.package
            ));
        }
    }
    let m = src.manifest_bytes();
    put_immutable(out, &layout::blob(&qrtz_modules::sha256_hex(&m)), &m)?;
    entry_for(out, team, &src.manifest(), src.tree(), None, None)
}

/// The current timestamp and index in `out`, if any.
pub fn current(out: &Path) -> Result<Option<(Timestamp, Index)>> {
    let tp = out.join(layout::timestamp());
    let Ok(tb) = std::fs::read(&tp) else {
        return Ok(None);
    };
    let t = Timestamp::parse(&tb).map_err(|e| e.to_string())?;
    let ip = out.join(layout::index(&t.index.sha256));
    let i = Index::parse(&std::fs::read(&ip).map_err(io(&ip))?).map_err(|e| e.to_string())?;
    Ok(Some((t, i)))
}

fn write_timestamp(
    out: &Path,
    keys: &Keys,
    index: &IndexRef,
    prev: Option<&Timestamp>,
    now: u64,
) -> Result<Timestamp> {
    let k = keys
        .timestamp
        .as_ref()
        .ok_or("no timestamp key (timestamp.key or QRTZ_PUBLISH_TIMESTAMP_KEY)")?;
    let t = Timestamp {
        kind: "timestamp".into(),
        version: prev.map_or(1, |p| p.version + 1),
        issued_ms: now,
        expires_ms: now + TIMESTAMP_DAYS * DAY_MS,
        index: index.clone(),
    };
    let b = canonical(&t);
    Timestamp::parse(&b).map_err(|e| e.to_string())?;
    // The signature goes first: a reader that sees the new timestamp finds its signature already there.
    put(out, &layout::sig(&layout::timestamp()), &sign(&b, &[k]))?;
    put(out, &layout::timestamp(), &b)?;
    Ok(t)
}

#[derive(Debug, Clone)]
pub struct Built {
    pub index_version: u64,
    pub index_sha256: String,
    pub timestamp_version: u64,
    pub added: Vec<String>,
}

/// `build`: add the spec's packages (and revocations) to the published index, sign a new index and timestamp.
pub fn build(out: &Path, spec: &Spec, keys: &Keys, now: u64) -> Result<Built> {
    if !is_team_id(&spec.team_id) {
        return Err("teamId: 10 uppercase letters or digits".into());
    }
    if highest_root(out).is_none() {
        return Err("no root in the output dir; run `qrtz-publish root` first".into());
    }
    let ik = keys
        .index
        .as_ref()
        .ok_or("no index key (index.key or QRTZ_PUBLISH_INDEX_KEY)")?;
    let prev = current(out)?;
    let mut packages = prev.as_ref().map(|(_, i)| i.packages.clone()).unwrap_or_default();
    let mut revoked = prev.as_ref().map(|(_, i)| i.revoked.clone()).unwrap_or_default();
    let mut added = vec![];
    let mut built = vec![];
    for p in &spec.packages {
        built.push(package_entry(out, &spec.team_id, p)?);
    }
    let mut pins = BTreeMap::new();
    for f in &spec.pinned_models {
        for m in qrtz_modules::models::parse_pinned(&std::fs::read(f).map_err(io(f))?).map_err(io(f))? {
            pins.insert(m.package.clone(), m);
        }
    }
    for m in &spec.models {
        let src = pins
            .get(&m.name)
            .ok_or(format!("{}: not in any pinnedModels file", m.name))?;
        built.push((
            m.name.clone(),
            src.version.clone(),
            model_entry(out, &spec.team_id, src, &m.dir)?,
        ));
    }
    for (name, version, e) in built {
        let slot = packages.entry(name.clone()).or_insert_with(BTreeMap::new);
        match slot.get(&version) {
            Some(have) if *have != e => {
                return Err(format!(
                    "{name}@{version}: already published with other content; bump the version"
                ))
            }
            Some(_) => {}
            None => {
                slot.insert(version.clone(), e);
                added.push(format!("{name}@{version}"));
            }
        }
    }
    for r in &spec.revoked {
        if !revoked.contains(r) {
            revoked.push(r.clone());
        }
    }
    // Every extension's ranges must be satisfiable by what is published.
    for (id, vs) in &packages {
        for (v, e) in vs {
            for (d, r) in &e.dependencies {
                let req = semver::VersionReq::parse(r).map_err(|x| format!("{id}@{v}: {d}@{r}: {x}"))?;
                let ok = packages.get(d).is_some_and(|x| {
                    x.keys()
                        .any(|dv| semver::Version::parse(dv).is_ok_and(|s| req.matches(&s)))
                });
                if !ok {
                    return Err(format!(
                        "{id}@{v} depends on {d}@{r}, and nothing published matches"
                    ));
                }
            }
        }
    }
    let index = Index {
        kind: "index".into(),
        version: prev.as_ref().map_or(1, |(_, i)| i.version + 1),
        issued_ms: now,
        expires_ms: now + INDEX_DAYS * DAY_MS,
        revoked,
        packages,
    };
    index.validate().map_err(|e| e.to_string())?;
    let b = canonical(&index);
    let sha = qrtz_modules::sha256_hex(&b);
    put_immutable(out, &layout::sig(&layout::index(&sha)), &sign(&b, &[ik]))?;
    put_immutable(out, &layout::index(&sha), &b)?;
    let t = write_timestamp(
        out,
        keys,
        &IndexRef {
            sha256: sha.clone(),
            size: b.len() as u64,
            version: index.version,
        },
        prev.as_ref().map(|(t, _)| t),
        now,
    )?;
    Ok(Built {
        index_version: index.version,
        index_sha256: sha,
        timestamp_version: t.version,
        added,
    })
}

/// `timestamp`: re-sign the timestamp for the current index (at least weekly; daily is the plan).
pub fn resign_timestamp(out: &Path, keys: &Keys, now: u64) -> Result<Timestamp> {
    let (t, _) = current(out)?.ok_or("nothing published yet")?;
    write_timestamp(out, keys, &t.index, Some(&t), now)
}

/// `pack`: a `.qrtzpkg` for one package version and everything it resolves to (against the published index):
/// every root, the timestamp and index, and the blobs (trees and files) of the whole set.
pub fn pack(out: &Path, name: &str, version: &str, dest: &Path) -> Result<Vec<String>> {
    use qrtz_modules::pkg::PkgRef;
    use qrtz_modules::qrtz_lock::{QrtzLock, Workspace};
    let (t, index) = current(out)?.ok_or("nothing published yet")?;
    index
        .entry(name, version)
        .ok_or(format!("{name}@{version}: not published"))?;
    let ws = Workspace {
        dependencies: BTreeMap::from([(name.to_owned(), format!("={version}"))]),
        ..Workspace::default()
    };
    // A pack carries exactly what is signed, revoked or not: the client's rule 2 refuses a revoked package from its
    // freshest list (the one in the file or a newer stored one), so resolve here as if nothing were revoked.
    let mut all = index.clone();
    all.revoked.clear();
    let r = qrtz_modules::resolve::resolve(&all, &ws, &QrtzLock::default(), &Default::default())
        .map_err(|e| e.to_string())?;
    let mut files: Vec<(String, PathBuf)> = vec![];
    for n in 1..=highest_root(out).unwrap_or(0) {
        for p in [layout::root(n), layout::sig(&layout::root(n))] {
            files.push((p.clone(), out.join(&p)));
        }
    }
    for p in [
        layout::timestamp(),
        layout::sig(&layout::timestamp()),
        layout::index(&t.index.sha256),
        layout::sig(&layout::index(&t.index.sha256)),
    ] {
        files.push((p.clone(), out.join(&p)));
    }
    let mut set = vec![];
    for p in r.packages.values() {
        set.push(PkgRef {
            name: p.name.clone(),
            version: p.version.clone(),
        });
        for b in p.entry.blobs() {
            let lp = layout::blob(&b.sha256);
            // ADR 0171: a model file isn't in the output dir; a pack (an offline install) takes the publisher's copy.
            let from = if b.mirrors.is_empty() {
                out.join(&lp)
            } else {
                model_blob_dir(out).join(&lp)
            };
            files.push((lp.clone(), from));
        }
    }
    for (_, f) in &files {
        if !f.exists() {
            return Err(format!("{}: missing from the output dir", f.display()));
        }
    }
    let target = PkgRef {
        name: name.into(),
        version: version.into(),
    };
    qrtz_modules::pkg::write(dest, &target, &set, &files).map_err(io(dest))?;
    Ok(set.iter().map(|p| format!("{}@{}", p.name, p.version)).collect())
}

/// Cache-Control of an immutable published object (everything but the timestamp pair).
pub const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Cache-Control of `v1/timestamp.json` and its signature.
pub const CACHE_TIMESTAMP: &str = "public, max-age=300";

/// One object to upload to modules.qrtz.ai.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upload {
    /// The bucket key (`v1/index/…`), also its path under `out`.
    pub key: String,
    pub size: u64,
    pub cache_control: &'static str,
}

/// `upload-list` (ADR 0171): every object of `out` in upload order, for an upload without rclone (the Cloudflare
/// connector, the dashboard). Immutable objects first, the timestamp's signature then the timestamp last, so a
/// client never sees a timestamp whose index or blobs aren't up. `ROOT1_SHA256` (a local note) is never listed, and
/// neither is anything outside `v1/` (model files never are in `out`: Hugging Face serves them). `timestamp_only`
/// lists the pair alone (the weekly re-sign).
pub fn upload_list(out: &Path, timestamp_only: bool) -> Result<Vec<Upload>> {
    let mut files = vec![];
    walk(&out.join("v1"), out, &mut files)?;
    let ts = layout::timestamp();
    let ts_sig = layout::sig(&ts);
    let mut list = vec![];
    let mut pair = vec![];
    for (key, p, _) in files {
        let size = std::fs::metadata(&p).map_err(io(&p))?.len();
        if key == ts || key == ts_sig {
            pair.push(Upload {
                key,
                size,
                cache_control: CACHE_TIMESTAMP,
            });
        } else if !timestamp_only {
            list.push(Upload {
                key,
                size,
                cache_control: CACHE_IMMUTABLE,
            });
        }
    }
    if !pair.iter().any(|u| u.key == ts) || !pair.iter().any(|u| u.key == ts_sig) {
        return Err(format!("{}: no signed timestamp (build first)", out.display()));
    }
    pair.sort_by_key(|u| u.key != ts_sig);
    list.extend(pair);
    Ok(list)
}

/// `model-files` (ADR 0171): the pinned files of one model package, `(Hugging Face URL, path, size, sha256)`, so the
/// publish script can download a set once to hash it (the index records the URL and the hash; no model byte is
/// uploaded anywhere).
pub fn model_files(pins: &[PathBuf], package: &str) -> Result<Vec<(String, String, u64, String)>> {
    for f in pins {
        for m in qrtz_modules::models::parse_pinned(&std::fs::read(f).map_err(io(f))?).map_err(io(f))? {
            if m.package == package {
                return Ok(m
                    .files
                    .iter()
                    .map(|x| (m.mirror_url(x), x.path.clone(), x.size, x.sha256.clone()))
                    .collect());
            }
        }
    }
    Err(format!("{package}: not in any pinned model file"))
}

/// `verify`: check `out` as a fresh client would (the pinned root, the root chain, a fresh timestamp, the index,
/// and every blob's size and sha256).
pub fn verify(out: &Path, pinned_root_sha256: &str, now: u64) -> Result<String> {
    use qrtz_modules::trust::{Mode, Trust};
    let r = |p: String| std::fs::read(out.join(&p)).map_err(|e| format!("{p}: {e}"));
    let mut t = Trust::bootstrap(
        Some(pinned_root_sha256),
        &r(layout::root(1))?,
        &r(layout::sig(&layout::root(1)))?,
    )
    .map_err(|e| e.to_string())?;
    let mut n = 2;
    while out.join(layout::root(n)).exists() {
        t.update_root(&r(layout::root(n))?, &r(layout::sig(&layout::root(n)))?)
            .map_err(|e| e.to_string())?;
        n += 1;
    }
    let ts = r(layout::timestamp())?;
    let stamp = Timestamp::parse(&ts).map_err(|e| e.to_string())?;
    let v = t
        .verify_pair(
            Mode::Network,
            &ts,
            &r(layout::sig(&layout::timestamp()))?,
            &r(layout::index(&stamp.index.sha256))?,
            &r(layout::sig(&layout::index(&stamp.index.sha256)))?,
            now,
        )
        .map_err(|e| e.to_string())?;
    let mut blobs = std::collections::BTreeSet::new();
    for vs in v.index.packages.values() {
        for e in vs.values() {
            for c in e.blobs() {
                // ADR 0171: a model file lives on Hugging Face, not here; its hash is in the signed tree.
                if !c.mirrors.is_empty() {
                    if out.join(layout::blob(&c.sha256)).exists() {
                        return Err(format!("{}: a model file in the output dir (ADR 0171)", c.sha256));
                    }
                    continue;
                }
                if !blobs.insert(c.sha256.clone()) {
                    continue;
                }
                let p = out.join(layout::blob(&c.sha256));
                let (n, h) = qrtz_modules::fsutil::sha256_file(&p).map_err(io(&p))?;
                if n != c.size || h != c.sha256 {
                    return Err(format!("{}: doesn't match the index", p.display()));
                }
            }
        }
    }
    let blobs = blobs.len();
    Ok(format!(
        "ok: root {}, timestamp {} (expires in {} h), index {} with {} packages, {blobs} blobs checked",
        t.root.version,
        v.timestamp.version,
        v.timestamp.expires_ms.saturating_sub(now) / 3_600_000,
        v.index.version,
        v.index.packages.values().map(|x| x.len()).sum::<usize>()
    ))
}

pub mod dry_run;
