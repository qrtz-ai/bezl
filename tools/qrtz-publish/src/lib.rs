//! qrtz-publish (ADR 0153): the release side of the signed module index.
//!
//! The output dir is exactly what goes to the bucket (`rclone sync <out> r2:qrtz-modules` and the B2 mirror):
//!
//! ```text
//! v1/root/<N>.root.json(.sig)      roots, never changed once written
//! v1/timestamp.json(.sig)          re-signed at least weekly (it expires after 7 days; the client allows 8)
//! v1/index/<sha256>.json(.sig)     one per publish, immutable, named by its hash
//! v1/blobs/sha256/<hex>            every file, or each 128 MiB chunk of a bigger one, immutable
//! ```
//!
//! Blobs and indexes are never overwritten or deleted; pulling a release is a new index with it in `revoked`.
pub mod keys;

use ed25519_dalek::SigningKey;
use keys::{sign, Keys};
use qrtz_modules::index::{
    canonical, chunk_sizes, key_id, layout, Chunk, Index, IndexFile, IndexRef, PackageEntry, PackageKind,
    PublicKey, Role, Roles, Root, Timestamp, CHUNK_SIZE, DAY_MS, MODULE_MANIFEST_FILE, PLATFORM,
};
use qrtz_modules::lock::{content_digest, PinnedRef};
use qrtz_modules::manifest::{designated_requirement, is_team_id, ModuleManifest};
use qrtz_modules::sideload::Revoked;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{Read, Write};
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
    /// `echo` / `nano` / `ctrl`, `model:<name>`, `xpc:<name>`.
    pub id: String,
    pub version: String,
    /// The built package dir: a module's dir holds `module.json` and its `.app` (or helper); a model set's dir
    /// holds its files.
    pub dir: PathBuf,
    #[serde(default)]
    pub min_os: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spec {
    /// The Developer ID team (also the publisher team of data packages).
    pub team_id: String,
    #[serde(default)]
    pub packages: Vec<SpecPackage>,
    /// Added to every list published before (the set never shrinks).
    #[serde(default)]
    pub revoked: Vec<Revoked>,
}

impl Spec {
    pub fn load(p: &Path) -> Result<Self> {
        let s: Spec = serde_json::from_slice(&std::fs::read(p).map_err(io(p))?).map_err(io(p))?;
        // Dirs are relative to the spec file.
        let base = p.parent().unwrap_or(Path::new("."));
        Ok(Spec {
            packages: s
                .packages
                .into_iter()
                .map(|mut x| {
                    if x.dir.is_relative() {
                        x.dir = base.join(&x.dir);
                    }
                    x
                })
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

/// Hash a file, writing it (or its chunks) as blobs into `out`.
fn blob_file(out: &Path, src: &Path) -> Result<(u64, String, Vec<Chunk>)> {
    use sha2::{Digest, Sha256};
    let size = std::fs::metadata(src).map_err(io(src))?.len();
    let mut f = std::fs::File::open(src).map_err(io(src))?;
    let mut whole = Sha256::new();
    let mut chunks = vec![];
    let tmp_dir = out.join("v1/blobs/.incoming");
    std::fs::create_dir_all(&tmp_dir).map_err(io(&tmp_dir))?;
    let mut buf = vec![0u8; 1 << 20];
    for (i, len) in chunk_sizes(size).into_iter().enumerate() {
        let tmp = tmp_dir.join(format!("{}-{i}", std::process::id()));
        let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(io(&tmp))?);
        let mut h = Sha256::new();
        let mut left = len;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let k = f.read(&mut buf[..want]).map_err(io(src))?;
            if k == 0 {
                return Err(format!("{}: changed while publishing", src.display()));
            }
            h.update(&buf[..k]);
            whole.update(&buf[..k]);
            w.write_all(&buf[..k]).map_err(io(&tmp))?;
            left -= k as u64;
        }
        w.flush().map_err(io(&tmp))?;
        drop(w);
        let hex = hex::encode(h.finalize());
        let dest = out.join(layout::blob(&hex));
        if dest.exists() {
            let _ = std::fs::remove_file(&tmp);
        } else {
            std::fs::create_dir_all(dest.parent().expect("parent")).map_err(io(&dest))?;
            std::fs::rename(&tmp, &dest).map_err(io(&dest))?;
        }
        chunks.push(Chunk {
            sha256: hex,
            size: len,
        });
    }
    let sha = hex::encode(whole.finalize());
    if size <= CHUNK_SIZE {
        chunks.clear();
    }
    Ok((size, sha, chunks))
}

fn kind_of(id: &str) -> Result<PackageKind> {
    Ok(if qrtz_modules::MODULE_IDS.contains(&id) {
        PackageKind::Module
    } else if id.starts_with("model:") {
        PackageKind::Data
    } else if id.starts_with("xpc:") {
        PackageKind::XpcService
    } else {
        return Err(format!("{id}: not a module, model:<name> or xpc:<name>"));
    })
}

/// One package from its built dir: blobs into `out`, the index entry back.
pub fn package_entry(out: &Path, team: &str, p: &SpecPackage) -> Result<PackageEntry> {
    let kind = kind_of(&p.id)?;
    let mut files = vec![];
    walk(&p.dir, &p.dir, &mut files)?;
    if files.is_empty() {
        return Err(format!("{}: empty", p.dir.display()));
    }
    let mut entries = vec![];
    for (rel, src, exec) in &files {
        let (size, sha256, chunks) = blob_file(out, src)?;
        entries.push(IndexFile {
            path: rel.clone(),
            size,
            sha256,
            executable: *exec,
            chunks,
        });
    }
    let (mut manifest_sha256, mut requires) = (None, vec![]);
    if kind == PackageKind::Module {
        let mp = p.dir.join(MODULE_MANIFEST_FILE);
        let bytes =
            std::fs::read(&mp).map_err(|_| format!("{}: a module dir needs module.json", p.dir.display()))?;
        let m = ModuleManifest::parse(&bytes).map_err(|e| format!("{}: {e}", mp.display()))?;
        if m.id != p.id || m.version != p.version {
            return Err(format!(
                "{}: module.json says {}@{}",
                p.dir.display(),
                m.id,
                m.version
            ));
        }
        if m.code.team_id != team {
            return Err(format!(
                "{}: module.json's team {} isn't the spec's {team}",
                p.id, m.code.team_id
            ));
        }
        manifest_sha256 = Some(qrtz_modules::sha256_hex(&bytes));
        requires = m
            .dependencies
            .iter()
            .map(|d| PinnedRef {
                id: d.id.clone(),
                version: d.version.clone(),
            })
            .collect();
        requires.sort();
    }
    let (bundle_id, dr) = if kind == PackageKind::XpcService {
        let b = format!("ai.qrtz.inference.{}", &p.id[4..]);
        let dr = designated_requirement(&b, team);
        (Some(b), Some(dr))
    } else {
        (None, None)
    };
    Ok(PackageEntry {
        kind,
        team_id: team.into(),
        sha256: content_digest(
            entries
                .iter()
                .map(|f| (f.path.as_str(), f.size, f.sha256.as_str())),
        ),
        size: entries.iter().map(|f| f.size).sum(),
        files: entries,
        bundle_id,
        designated_requirement: dr,
        manifest_sha256,
        requires,
        platform: PLATFORM.into(),
        min_os: p.min_os.clone(),
    })
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
    for p in &spec.packages {
        let e = package_entry(out, &spec.team_id, p)?;
        let slot = packages.entry(p.id.clone()).or_insert_with(BTreeMap::new);
        match slot.get(&p.version) {
            Some(have) if *have != e => {
                return Err(format!(
                    "{}@{}: already published with other content; bump the version",
                    p.id, p.version
                ))
            }
            Some(_) => {}
            None => {
                slot.insert(p.version.clone(), e);
                added.push(format!("{}@{}", p.id, p.version));
            }
        }
    }
    for r in &spec.revoked {
        if !revoked.contains(r) {
            revoked.push(r.clone());
        }
    }
    // Every module's requires must be published, with matching pins.
    for (id, vs) in &packages {
        for (v, e) in vs {
            for r in &e.requires {
                if !packages.get(&r.id).is_some_and(|x| x.contains_key(&r.version)) {
                    return Err(format!(
                        "{id}@{v} requires {}@{}, which isn't published",
                        r.id, r.version
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
    let _ = std::fs::remove_dir(out.join("v1/blobs/.incoming"));
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

/// `pack`: a `.qrtzpkg` for one module version: every root, the timestamp and index, and the blobs of the module
/// and everything it requires.
pub fn pack(out: &Path, module: &str, version: &str, dest: &Path) -> Result<()> {
    let (t, index) = current(out)?.ok_or("nothing published yet")?;
    let e = index
        .entry(module, version)
        .ok_or(format!("{module}@{version}: not published"))?;
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
    let mut entries = vec![e];
    for r in &e.requires {
        entries.push(
            index
                .entry(&r.id, &r.version)
                .ok_or(format!("{}@{}: not published", r.id, r.version))?,
        );
    }
    for e in entries {
        for c in e.blobs() {
            let p = layout::blob(&c.sha256);
            files.push((p.clone(), out.join(&p)));
        }
    }
    for (_, f) in &files {
        if !f.exists() {
            return Err(format!("{}: missing from the output dir", f.display()));
        }
    }
    qrtz_modules::pkg::write(dest, module, version, &files).map_err(io(dest))
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
    let mut blobs = 0;
    for vs in v.index.packages.values() {
        for e in vs.values() {
            for c in e.blobs() {
                let p = out.join(layout::blob(&c.sha256));
                let (n, h) = qrtz_modules::fsutil::sha256_file(&p).map_err(io(&p))?;
                if n != c.size || h != c.sha256 {
                    return Err(format!("{}: doesn't match the index", p.display()));
                }
                blobs += 1;
            }
        }
    }
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
