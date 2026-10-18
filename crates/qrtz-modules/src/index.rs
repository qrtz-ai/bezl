//! The signed module index (ADR 0153, a TUF-lite successor to ADR 0046 §4): three canonical JSON documents, each
//! with a detached Ed25519 signature file, and the URL layout they are served under.
//!
//! - `root`: the keys of the `root`, `timestamp` and `index` roles, a threshold per role, a version and an expiry
//!   (at most 366 days). Root `N+1` must be signed by a threshold of root `N`'s root keys *and* of its own.
//! - `timestamp`: `{version, issuedMs, expiresMs, index: {sha256, size, version}}`; expires at most 8 days after it
//!   was issued (the ADR 0046 clamp). This is the freshness of everything, revocations included.
//! - `index`: content-addressed (`/v1/index/<sha256>.json`), `{version, issuedMs, expiresMs, revoked, packages}`.
//!   `revoked` is the revocation list (it replaces ADR 0046's separate denylist). `packages` maps an id (`echo`,
//!   `model:parakeet-tdt-v2`, `xpc:voice`) to its exact versions, each listing every file with its size, sha256
//!   and, past [`CHUNK_SIZE`], its chunks.
//!
//! Every document is canonical JSON (keys sorted, no whitespace, integers only); a document whose bytes aren't
//! canonical is refused, so there is exactly one byte string for each signed value. Signatures are over the exact
//! bytes served. Hosts are transport only: every byte is checked against a signed hash before it is used.
use crate::sideload::Revoked;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Every URL starts here, on the primary and on each mirror.
pub const LAYOUT_VERSION: &str = "v1";
/// Files above this are split into chunks of this size (the last may be shorter). 128 MiB keeps every object under
/// Cloudflare's 512 MB cache limit and makes a resume lose at most one chunk.
pub const CHUNK_SIZE: u64 = 128 * 1024 * 1024;
/// The ADR 0046 clamp: a timestamp is valid for at most 8 days.
pub const MAX_TIMESTAMP_VALIDITY_MS: u64 = 8 * DAY_MS;
/// Roots and indexes are valid for at most a year (plus a day of slack for re-signing).
pub const MAX_ROOT_VALIDITY_MS: u64 = 366 * DAY_MS;
pub const MAX_INDEX_VALIDITY_MS: u64 = 366 * DAY_MS;
pub const DAY_MS: u64 = 86_400_000;
/// Root, timestamp and signature files are small; the index is at most 4 MiB.
pub const MAX_META_BYTES: usize = 256 * 1024;
pub const MAX_INDEX_BYTES: usize = 4 * 1024 * 1024;
/// A package (all its files) is at most 4 GiB; the largest model set today is 845 MB.
pub const MAX_PACKAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_FILES: usize = 20_000;

/// URL paths relative to a host base (`https://modules.qrtz.ai`). No query strings, ids or tokens: names and
/// versions appear only inside the signed index.
pub mod layout {
    use super::LAYOUT_VERSION;
    pub fn root(n: u64) -> String {
        format!("{LAYOUT_VERSION}/root/{n}.root.json")
    }
    pub fn timestamp() -> String {
        format!("{LAYOUT_VERSION}/timestamp.json")
    }
    pub fn index(sha256: &str) -> String {
        format!("{LAYOUT_VERSION}/index/{sha256}.json")
    }
    pub fn blob(sha256: &str) -> String {
        format!("{LAYOUT_VERSION}/blobs/sha256/{sha256}")
    }
    /// The detached signature of a document.
    pub fn sig(doc_path: &str) -> String {
        format!("{doc_path}.sig")
    }
    /// True for exactly the paths above (used by `.qrtzpkg` readers and the publisher's output check).
    pub fn is_valid(p: &str) -> bool {
        let Some(rest) = p.strip_prefix(&format!("{LAYOUT_VERSION}/")) else {
            return false;
        };
        let rest = rest.strip_suffix(".sig").unwrap_or(rest);
        if rest == "timestamp.json" {
            return true;
        }
        if let Some(n) = rest
            .strip_prefix("root/")
            .and_then(|r| r.strip_suffix(".root.json"))
        {
            return !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && !n.starts_with('0');
        }
        if let Some(h) = rest.strip_prefix("index/").and_then(|r| r.strip_suffix(".json")) {
            return crate::manifest::is_sha256(h);
        }
        if let Some(h) = rest.strip_prefix("blobs/sha256/") {
            return crate::manifest::is_sha256(h) && !p.ends_with(".sig");
        }
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    #[error("{0}: not canonical json (keys sorted, no whitespace)")]
    NotCanonical(&'static str),
    #[error("{0}: {1}")]
    Invalid(&'static str, String),
    #[error("{0}: too large")]
    TooLarge(&'static str),
    #[error("{0}: signature threshold not met")]
    Signature(&'static str),
    #[error("{what}: expired")]
    Expired { what: &'static str },
    #[error("{what}: version {got} is older than the stored {stored} (rollback or freeze)")]
    Rollback {
        what: &'static str,
        got: u64,
        stored: u64,
    },
    #[error("{what}: version {version} was already seen with other content")]
    Equivocation { what: &'static str, version: u64 },
    #[error("{0}: doesn't match what the timestamp names")]
    TimestampMismatch(&'static str),
    #[error("root: sha256 differs from the root pinned in this build")]
    UnpinnedRoot,
    #[error("no root is pinned in this build (docs/modules/hosting-setup.md)")]
    NoPinnedRoot,
}

/// Canonical bytes of `v`: serialized through `serde_json::Value`, whose maps are sorted (no `preserve_order`).
pub fn canonical<T: Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(&serde_json::to_value(v).expect("plain json")).expect("plain json")
}

/// Parse a document, refusing bytes that aren't the canonical form of what they decode to.
pub fn parse_canonical<T: DeserializeOwned>(
    what: &'static str,
    bytes: &[u8],
    max: usize,
) -> Result<T, IndexError> {
    if bytes.len() > max {
        return Err(IndexError::TooLarge(what));
    }
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| IndexError::Invalid(what, e.to_string()))?;
    if serde_json::to_vec(&v).expect("plain json") != bytes || has_float(&v) {
        return Err(IndexError::NotCanonical(what));
    }
    serde_json::from_value(v).map_err(|e| IndexError::Invalid(what, e.to_string()))
}

fn has_float(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Number(n) => n.is_f64(),
        serde_json::Value::Array(a) => a.iter().any(has_float),
        serde_json::Value::Object(o) => o.values().any(has_float),
        _ => false,
    }
}

/// One signature in a `.sig` file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Signature {
    /// sha256 hex of the 32-byte public key.
    pub keyid: String,
    /// 64-byte Ed25519 signature, hex.
    pub sig: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Signatures {
    pub signatures: Vec<Signature>,
}

impl Signatures {
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        parse_canonical("signature", bytes, MAX_META_BYTES)
    }
}

/// The key id of an Ed25519 public key.
pub fn key_id(public: &[u8; 32]) -> String {
    crate::sha256_hex(public)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicKey {
    /// Always `ed25519`.
    pub scheme: String,
    /// 32 bytes, hex.
    pub public: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Role {
    pub keyids: Vec<String>,
    pub threshold: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Roles {
    pub root: Role,
    pub timestamp: Role,
    pub index: Role,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Root {
    /// Always `root`, so a document can't be replayed as another role's.
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u64,
    pub issued_ms: u64,
    pub expires_ms: u64,
    pub keys: BTreeMap<String, PublicKey>,
    pub roles: Roles,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexRef {
    pub sha256: String,
    pub size: u64,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Timestamp {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u64,
    pub issued_ms: u64,
    pub expires_ms: u64,
    pub index: IndexRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    /// echo, nano, ctrl: a signed `.app` plus its `module.json`.
    Module,
    /// A model set: read-only files.
    Data,
    /// Shared code in its own sandboxed XPC service (ADR 0144 §2).
    XpcService,
    /// The qrtz shell itself (reserved; shell updates come later).
    Shell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Chunk {
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexFile {
    /// Relative to the package dir; no `..`, no absolute paths, no backslashes.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// The file gets 0500 instead of 0400 (a `.app`'s `Contents/MacOS/*`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executable: bool,
    /// Present exactly when `size > CHUNK_SIZE`: every chunk but the last is `CHUNK_SIZE` long.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunks: Vec<Chunk>,
}

impl IndexFile {
    /// The blobs this file is served as: itself, or its chunks.
    pub fn blobs(&self) -> Vec<Chunk> {
        if self.chunks.is_empty() {
            vec![Chunk {
                sha256: self.sha256.clone(),
                size: self.size,
            }]
        } else {
            self.chunks.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageEntry {
    pub kind: PackageKind,
    /// Developer ID team (code) or the publisher's team (data). The lockfile pins it.
    pub team_id: String,
    /// [`crate::lock::content_digest`] of `files`: the hash a module manifest pins its dependencies by.
    pub sha256: String,
    /// Sum of the file sizes.
    pub size: u64,
    pub files: Vec<IndexFile>,
    /// `xpc-service` only (with `designatedRequirement`); a module's identity is in its `module.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designated_requirement: Option<String>,
    /// Module only: sha256 of its `module.json`, which must be one of `files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
    /// Module only: every dependency its `module.json` pins (optional ones too), so the download size can be shown
    /// before anything is fetched. The installer checks it equals the manifest's list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<crate::lock::PinnedRef>,
    /// `macos-arm64`.
    pub platform: String,
    /// The lowest macOS this runs on (`14.2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_os: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Index {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u64,
    pub issued_ms: u64,
    pub expires_ms: u64,
    #[serde(default)]
    pub revoked: Vec<Revoked>,
    /// id → exact version → entry.
    pub packages: BTreeMap<String, BTreeMap<String, PackageEntry>>,
}

/// The manifest file inside a module package.
pub const MODULE_MANIFEST_FILE: &str = "module.json";
pub const PLATFORM: &str = "macos-arm64";

fn bad(what: &'static str, why: impl Into<String>) -> IndexError {
    IndexError::Invalid(what, why.into())
}

fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 1024
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains('\0')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

fn check_role(what: &'static str, keys: &BTreeMap<String, PublicKey>, r: &Role) -> Result<(), IndexError> {
    let mut ids = r.keyids.clone();
    ids.sort();
    ids.dedup();
    if ids.len() != r.keyids.len() {
        return Err(bad(what, "a key is listed twice"));
    }
    if r.threshold == 0 || r.threshold as usize > r.keyids.len() {
        return Err(bad(what, "threshold must be between 1 and the number of keys"));
    }
    for k in &r.keyids {
        if !keys.contains_key(k) {
            return Err(bad(what, format!("unknown key {k}")));
        }
    }
    Ok(())
}

impl Root {
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        let r: Self = parse_canonical("root", bytes, MAX_META_BYTES)?;
        r.validate()?;
        Ok(r)
    }
    pub fn validate(&self) -> Result<(), IndexError> {
        if self.kind != "root" {
            return Err(bad("root", "type must be root"));
        }
        if self.version == 0 {
            return Err(bad("root", "versions start at 1"));
        }
        if self.expires_ms <= self.issued_ms || self.expires_ms - self.issued_ms > MAX_ROOT_VALIDITY_MS {
            return Err(bad("root", "expires must be after issued, within 366 days"));
        }
        for (id, k) in &self.keys {
            let raw = decode_key(&k.public).ok_or_else(|| bad("root", format!("key {id}: 32 bytes hex")))?;
            if k.scheme != "ed25519" || key_id(&raw) != *id {
                return Err(bad("root", format!("key {id}: ed25519, keyid = sha256(public)")));
            }
        }
        check_role("root.roles.root", &self.keys, &self.roles.root)?;
        check_role("root.roles.timestamp", &self.keys, &self.roles.timestamp)?;
        check_role("root.roles.index", &self.keys, &self.roles.index)?;
        Ok(())
    }

    /// Detached signatures over `bytes` meet `role`'s threshold, counting each key once.
    pub fn verify_role(
        &self,
        what: &'static str,
        role: &Role,
        bytes: &[u8],
        sigs: &Signatures,
    ) -> Result<(), IndexError> {
        let mut good = std::collections::BTreeSet::new();
        for s in &sigs.signatures {
            if !role.keyids.contains(&s.keyid) || good.contains(&s.keyid) {
                continue;
            }
            let Some(k) = self.keys.get(&s.keyid).and_then(|k| decode_key(&k.public)) else {
                continue;
            };
            let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(&k) else {
                continue;
            };
            let Some(sig) = hex::decode(&s.sig)
                .ok()
                .and_then(|b| <[u8; 64]>::try_from(b).ok())
            else {
                continue;
            };
            if vk
                .verify_strict(bytes, &ed25519_dalek::Signature::from_bytes(&sig))
                .is_ok()
            {
                good.insert(s.keyid.clone());
            }
        }
        if good.len() >= role.threshold as usize {
            Ok(())
        } else {
            Err(IndexError::Signature(what))
        }
    }
}

pub fn decode_key(h: &str) -> Option<[u8; 32]> {
    hex::decode(h).ok().and_then(|b| b.try_into().ok())
}

impl Timestamp {
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        let t: Self = parse_canonical("timestamp", bytes, MAX_META_BYTES)?;
        if t.kind != "timestamp" {
            return Err(bad("timestamp", "type must be timestamp"));
        }
        if t.expires_ms <= t.issued_ms || t.expires_ms - t.issued_ms > MAX_TIMESTAMP_VALIDITY_MS {
            return Err(bad("timestamp", "expires must be after issued, within 8 days"));
        }
        if !crate::manifest::is_sha256(&t.index.sha256) || t.index.size as usize > MAX_INDEX_BYTES {
            return Err(bad("timestamp", "index sha256 or size"));
        }
        Ok(t)
    }
}

impl Index {
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        let i: Self = parse_canonical("index", bytes, MAX_INDEX_BYTES)?;
        i.validate()?;
        Ok(i)
    }

    pub fn validate(&self) -> Result<(), IndexError> {
        if self.kind != "index" {
            return Err(bad("index", "type must be index"));
        }
        if self.expires_ms <= self.issued_ms || self.expires_ms - self.issued_ms > MAX_INDEX_VALIDITY_MS {
            return Err(bad("index", "expires must be after issued, within 366 days"));
        }
        for r in &self.revoked {
            for v in &r.versions {
                crate::manifest::exact_version("index.revoked", v)
                    .map_err(|e| bad("index", e.to_string()))?;
            }
        }
        for (id, versions) in &self.packages {
            for (v, e) in versions {
                crate::manifest::exact_version("index.packages", v)
                    .map_err(|e| bad("index", e.to_string()))?;
                e.validate(id)
                    .map_err(|why| bad("index", format!("{id}@{v}: {why}")))?;
            }
        }
        Ok(())
    }

    pub fn entry(&self, id: &str, version: &str) -> Option<&PackageEntry> {
        self.packages.get(id).and_then(|v| v.get(version))
    }

    /// The highest version of `id` that isn't revoked.
    pub fn latest(&self, id: &str) -> Option<(&str, &PackageEntry)> {
        self.packages
            .get(id)?
            .iter()
            .filter(|(v, _)| {
                !self
                    .revoked
                    .iter()
                    .any(|r| r.package == id && (r.versions.is_empty() || r.versions.contains(v)))
            })
            .max_by_key(|(v, _)| semver::Version::parse(v).ok())
            .map(|(v, e)| (v.as_str(), e))
    }
}

impl PackageEntry {
    fn validate(&self, id: &str) -> Result<(), String> {
        let prefix_ok = match self.kind {
            PackageKind::Module => crate::MODULE_IDS.contains(&id),
            PackageKind::Data => id.starts_with("model:"),
            PackageKind::XpcService => id.starts_with("xpc:"),
            PackageKind::Shell => id == "shell",
        };
        if !prefix_ok {
            return Err(format!("kind {:?} doesn't match the id", self.kind));
        }
        if !crate::manifest::is_team_id(&self.team_id) || !crate::manifest::is_sha256(&self.sha256) {
            return Err("teamId or sha256 malformed".into());
        }
        if self.platform != PLATFORM {
            return Err(format!("platform must be {PLATFORM}"));
        }
        if self.files.is_empty() || self.files.len() > MAX_FILES {
            return Err("files: 1 to 20000".into());
        }
        let mut paths = std::collections::BTreeSet::new();
        let mut total = 0u64;
        for f in &self.files {
            if !safe_rel(&f.path) || !paths.insert(f.path.as_str()) {
                return Err(format!("{:?}: unsafe or repeated path", f.path));
            }
            if !crate::manifest::is_sha256(&f.sha256) {
                return Err(format!("{}: sha256", f.path));
            }
            if f.size > CHUNK_SIZE {
                let n = f.size.div_ceil(CHUNK_SIZE) as usize;
                if f.chunks.len() != n {
                    return Err(format!("{}: {n} chunks expected", f.path));
                }
                for (i, c) in f.chunks.iter().enumerate() {
                    let want = if i + 1 < n {
                        CHUNK_SIZE
                    } else {
                        f.size - CHUNK_SIZE * (n as u64 - 1)
                    };
                    if c.size != want || !crate::manifest::is_sha256(&c.sha256) {
                        return Err(format!("{}: chunk {i}", f.path));
                    }
                }
            } else if !f.chunks.is_empty() {
                return Err(format!("{}: chunks only above {CHUNK_SIZE} bytes", f.path));
            }
            total = total.checked_add(f.size).ok_or("size overflow")?;
        }
        for p in &paths {
            let dir = format!("{p}/");
            if paths
                .range(dir.as_str()..)
                .next()
                .is_some_and(|q| q.starts_with(&dir))
            {
                return Err(format!("{p:?} is both a file and a directory"));
            }
        }
        if total != self.size || total > MAX_PACKAGE_BYTES {
            return Err("size must be the sum of the files, at most 4 GiB".into());
        }
        let digest = crate::lock::content_digest(
            self.files
                .iter()
                .map(|f| (f.path.as_str(), f.size, f.sha256.as_str())),
        );
        if digest != self.sha256 {
            return Err("sha256 must be the content digest of files".into());
        }
        let code = self.kind == PackageKind::XpcService;
        if code != self.bundle_id.is_some() || code != self.designated_requirement.is_some() {
            return Err("bundleId and designatedRequirement only (and always) on xpc-service".into());
        }
        if code {
            let name = id.trim_start_matches("xpc:");
            let b = format!("ai.qrtz.inference.{name}");
            if self.bundle_id.as_deref() != Some(&b)
                || self.designated_requirement.as_deref()
                    != Some(&crate::manifest::designated_requirement(&b, &self.team_id))
            {
                return Err(
                    "xpc-service identity must be the Developer ID requirement for its bundle id".into(),
                );
            }
        }
        match (self.kind, &self.manifest_sha256) {
            (PackageKind::Module, Some(h)) => {
                if !self
                    .files
                    .iter()
                    .any(|f| f.path == MODULE_MANIFEST_FILE && &f.sha256 == h)
                {
                    return Err("manifestSha256 must be the module.json in files".into());
                }
            }
            (PackageKind::Module, None) => return Err("a module names its manifestSha256".into()),
            (_, None) if !self.requires.is_empty() => return Err("requires is for modules".into()),
            (_, Some(_)) => return Err("manifestSha256 is for modules".into()),
            _ => {}
        }
        Ok(())
    }

    /// Every blob this package is served as, in file order (a blob shared by two files is listed once).
    pub fn blobs(&self) -> Vec<Chunk> {
        let mut seen = std::collections::BTreeSet::new();
        self.files
            .iter()
            .flat_map(IndexFile::blobs)
            .filter(|c| seen.insert(c.sha256.clone()))
            .collect()
    }
}

/// Split `size` bytes into the chunk sizes the index uses.
pub fn chunk_sizes(size: u64) -> Vec<u64> {
    if size <= CHUNK_SIZE {
        return vec![size];
    }
    let n = size.div_ceil(CHUNK_SIZE);
    (0..n)
        .map(|i| {
            if i + 1 < n {
                CHUNK_SIZE
            } else {
                size - CHUNK_SIZE * (n - 1)
            }
        })
        .collect()
}
