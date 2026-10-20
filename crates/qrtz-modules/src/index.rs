//! The signed package index (ADR 0153, a TUF-lite successor to ADR 0046 §4; package format from ADR 0157): three
//! canonical JSON documents, each with a detached Ed25519 signature file, and the URL layout they are served under.
//!
//! - `root`: the keys of the `root`, `timestamp` and `index` roles, a threshold per role, a version and an expiry
//!   (at most 366 days). Root `N+1` must be signed by a threshold of root `N`'s root keys *and* of its own.
//! - `timestamp`: `{version, issuedMs, expiresMs, index: {sha256, size, version}}`; expires at most 8 days after it
//!   was issued (the ADR 0046 clamp). This is the freshness of everything, revocations included.
//! - `index`: content-addressed (`/v1/index/<sha256>.json`), `{version, issuedMs, expiresMs, revoked, packages}`.
//!   `revoked` is the revocation list (it replaces ADR 0046's separate denylist). `packages` maps a package name
//!   (`echo`, `inference.voice`, `model.parakeet-tdt-v2-en`) to its exact versions. Each entry is its package's
//!   `qrtz.json` facts (kind, dependency ranges, code identity) plus its **tree**: every file with its size, sha256,
//!   executable bit and any extra mirror URLs. The entry's `sha256` is the sha256 of the tree's canonical bytes,
//!   which are also served as a blob, so a frozen lockfile can fetch a package by that one hash (ADR 0157 §4).
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
/// The ADR 0046 clamp: a timestamp is valid for at most 8 days.
pub const MAX_TIMESTAMP_VALIDITY_MS: u64 = 8 * DAY_MS;
/// Roots and indexes are valid for at most a year (plus a day of slack for re-signing).
pub const MAX_ROOT_VALIDITY_MS: u64 = 366 * DAY_MS;
pub const MAX_INDEX_VALIDITY_MS: u64 = 366 * DAY_MS;
pub const DAY_MS: u64 = 86_400_000;
/// Root, timestamp and signature files are small; the index is at most 4 MiB.
pub const MAX_META_BYTES: usize = 256 * 1024;
pub const MAX_INDEX_BYTES: usize = 4 * 1024 * 1024;
/// A package (all its files) is at most 4 GiB; the largest model set today is 626 MB.
pub const MAX_PACKAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_FILES: usize = 20_000;

/// URL paths relative to a host base (the fixed `index` GitHub release, QRT-297). No query strings, ids or tokens:
/// names and versions appear only inside the signed index.
pub mod layout {
    use super::LAYOUT_VERSION;
    /// QRT-297: a GitHub release holds flat asset names, no directories. A host base that is a release download
    /// base (`https://github.com/<owner>/<repo>/releases/download/<tag>`) serves layout path `v1/a/b` as the asset
    /// `v1.a.b`; any other base (a loopback test server, a local dir) serves the path as it is. The bytes are
    /// checked against the signed index either way, so the name only decides where to look.
    pub fn on_host(base: &str, path: &str) -> String {
        if is_release_base(base) {
            release_asset(path)
        } else {
            path.to_owned()
        }
    }
    /// The flat GitHub release asset name of a layout path (`v1/blobs/sha256/<hex>` → `v1.blobs.sha256.<hex>`).
    pub fn release_asset(path: &str) -> String {
        path.replace('/', ".")
    }
    /// True for `https://github.com/<owner>/<repo>/releases/download/<tag>` (no trailing slash, query or user).
    pub fn is_release_base(base: &str) -> bool {
        let Some(rest) = base.strip_prefix("https://github.com/") else {
            return false;
        };
        let parts: Vec<&str> = rest.split('/').collect();
        parts.len() == 5
            && parts[2] == "releases"
            && parts[3] == "download"
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
    }
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
    /// True for exactly the paths above (used by `.qrtzpkg` readers and the publisher's output check). Blobs are
    /// whole files (and package trees), never chunks: a big file is fetched with parallel range requests (ADR 0157).
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

/// What a package is (ADR 0157 §1). The name says it too: `model.*` is a model, `inference.*` a service, and an
/// extension is one of [`crate::EXTENSION_NAMES`]; the index refuses an entry whose kind and name disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    /// echo, nano, ctrl: a helper qrtz spawns itself, inside qrtz's own TCC identity (ADR 0157 §2).
    Extension,
    /// A sandboxed XPC service (`inference.voice`): no network, no TCC, reached only over XPC.
    Service,
    /// Data only (`model.parakeet-tdt-v2-en`): read-only files, readable only by the services that declare them.
    Model,
    /// The qrtz shell itself (reserved; shell updates come later).
    Shell,
}

impl PackageKind {
    /// Code runs from it (Apple signature checks and the revocation freshness rule apply).
    pub fn has_code(self) -> bool {
        matches!(self, Self::Extension | Self::Service | Self::Shell)
    }
}

/// One file of a package tree.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexFile {
    /// Relative to the package dir; no `..`, no absolute paths, no backslashes.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// The file gets 0500 instead of 0400 (a helper binary, a `.xpc`'s `Contents/MacOS/*`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executable: bool,
    /// Extra places the same bytes can be fetched from, after the module hosts (ADR 0157 §6: a model's pinned
    /// Hugging Face URL). Absolute `https://` URLs; checked by the same sha256 through the same fetcher, and the
    /// shell only uses the ones on its pinned mirror-host list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mirrors: Vec<String>,
}

/// A package's file list. Its canonical bytes are served as `v1/blobs/sha256/<sha256 of them>` and that hash is
/// the package's `sha256` everywhere (index entry, `qrtz.lock`, `.qrtzpkg`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tree {
    /// Sorted by path.
    pub files: Vec<IndexFile>,
}

impl Tree {
    pub fn new(mut files: Vec<IndexFile>) -> Self {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Self { files }
    }
    pub fn bytes(&self) -> Vec<u8> {
        canonical(self)
    }
    pub fn sha256(&self) -> String {
        crate::sha256_hex(&self.bytes())
    }
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        let t: Self = parse_canonical("tree", bytes, MAX_INDEX_BYTES)?;
        t.validate().map_err(|why| bad("tree", why))?;
        Ok(t)
    }

    /// Paths safe and unique, sorted, sizes summed within bounds, hashes well formed, mirrors `https://`.
    pub fn validate(&self) -> Result<(), String> {
        if self.files.is_empty() || self.files.len() > MAX_FILES {
            return Err("files: 1 to 20000".into());
        }
        let mut paths = std::collections::BTreeSet::new();
        let mut total = 0u64;
        let mut prev: Option<&str> = None;
        for f in &self.files {
            if !safe_rel(&f.path) || !paths.insert(f.path.as_str()) {
                return Err(format!("{:?}: unsafe or repeated path", f.path));
            }
            if prev.is_some_and(|p| p >= f.path.as_str()) {
                return Err("files must be sorted by path".into());
            }
            prev = Some(&f.path);
            if !crate::manifest::is_sha256(&f.sha256) {
                return Err(format!("{}: sha256", f.path));
            }
            for m in &f.mirrors {
                if !is_mirror_url(m) {
                    return Err(format!("{}: mirror {m:?} must be a plain https URL", f.path));
                }
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
        if total > MAX_PACKAGE_BYTES {
            return Err("a package is at most 4 GiB".into());
        }
        Ok(())
    }

    pub fn file(&self, path: &str) -> Option<&IndexFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Every distinct blob of the package (a blob shared by two files is listed once), with its mirrors.
    pub fn blobs(&self) -> Vec<BlobRef> {
        let mut out: BTreeMap<&str, BlobRef> = BTreeMap::new();
        for f in &self.files {
            let b = out.entry(&f.sha256).or_insert_with(|| BlobRef {
                sha256: f.sha256.clone(),
                size: f.size,
                mirrors: vec![],
            });
            for m in &f.mirrors {
                if !b.mirrors.contains(m) {
                    b.mirrors.push(m.clone());
                }
            }
        }
        out.into_values().collect()
    }
}

/// One blob to fetch: its hash and size (what is checked), and the extra URLs it may come from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlobRef {
    pub sha256: String,
    pub size: u64,
    pub mirrors: Vec<String>,
}

/// ADR 0171: `https://huggingface.co/<owner>/<repo>/resolve/<40-hex commit>/<path>`: a model file's only source,
/// pinned to an exact commit (never a branch like `main`, whose bytes can change under the signed hash).
pub fn is_pinned_hugging_face_url(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("https://huggingface.co/") else {
        return false;
    };
    let parts: Vec<&str> = rest.splitn(5, '/').collect();
    is_mirror_url(u)
        && parts.len() == 5
        && !parts[0].is_empty()
        && !parts[1].is_empty()
        && parts[2] == "resolve"
        && parts[3].len() == 40
        && parts[3]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && !parts[4].is_empty()
}

/// `https://host/path` with no user info, query, fragment, whitespace or `..`.
pub fn is_mirror_url(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("https://") else {
        return false;
    };
    let Some((host, path)) = rest.split_once('/') else {
        return false;
    };
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && u.len() <= 2048
        && !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"?#@\\".contains(&b))
        && path.split('/').all(|c| c != "..")
}

/// An index entry: one exact version of one package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageEntry {
    pub kind: PackageKind,
    /// The Developer ID team that signed the code (extensions and services), or the publisher's team (models).
    pub team_id: String,
    /// sha256 of the package tree's canonical bytes (also served as a blob).
    pub sha256: String,
    /// Sum of the file sizes.
    pub size: u64,
    /// The tree, inline, so a plan can show sizes before anything is fetched.
    pub files: Vec<IndexFile>,
    /// sha256 of the package's `qrtz.json`, which must be one of `files`.
    pub manifest_sha256: String,
    /// The `qrtz.json` dependency ranges, copied here so resolution needs only the index. The installer checks the
    /// staged `qrtz.json` says the same.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    /// Extension: the helper's code signing identifier (`ai.qrtz.echo`); service: its bundle id
    /// (`ai.qrtz.inference.voice`). A package signing identity, never a TCC identity (ADR 0154 §1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    /// Extension and service only: exactly [`crate::manifest::designated_requirement`] of `identifier`, `teamId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designated_requirement: Option<String>,
    /// Model only: what it is for (`asr`, `diarization`, `echo-cancellation`, `decision`). An engine swap may only
    /// replace a model with one of the same task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Service only: the models it may be handed (ADR 0157 §7).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// `macos-arm64`.
    pub platform: String,
    /// The lowest macOS this runs on (`14.2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_os: Option<String>,
    /// Extension and service only, always (ADR 0170): Apple's notarization of exactly this code, recorded by the
    /// publisher once `notarytool` answered Accepted and signed with the rest of the entry. The installer checks it
    /// offline; it never asks Gatekeeper online.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notarization: Option<Notarization>,
}

/// ADR 0170 (Bozniak's model): the notarization result a signed index carries for one package's code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Notarization {
    /// The notary service's submission id (a UUID, `notarytool submit` → `id`).
    pub submission_id: String,
    /// `notarytool`'s verdict. Only `Accepted` is ever published or installed.
    pub status: String,
    /// sha256 of the exact file that was submitted, which must be one of the package's `files`: an extension's
    /// `extension.entry` executable, or a service bundle's main executable (`<bundle>/Contents/MacOS/<name>`).
    pub code_sha256: String,
    /// The notarized code directory hash (`codesign -dvvv` → `CDHash=`, 40 lowercase hex). On macOS the installer
    /// reads the staged code's own cdhash and requires this one.
    pub cdhash: String,
}

pub const NOTARIZATION_ACCEPTED: &str = "Accepted";

impl Notarization {
    /// Well formed and Accepted.
    pub fn check(&self) -> Result<(), String> {
        let hex = |s: &str, n: usize| {
            s.len() == n
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let uuid = self.submission_id.len() == 36
            && self.submission_id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            });
        if !uuid {
            return Err("notarization.submissionId must be a UUID".into());
        }
        if self.status != NOTARIZATION_ACCEPTED {
            return Err(format!("notarization.status is {:?}, not Accepted", self.status));
        }
        if !hex(&self.code_sha256, 64) {
            return Err("notarization.codeSha256 must be a sha256".into());
        }
        if !hex(&self.cdhash, 40) {
            return Err("notarization.cdhash must be 40 lowercase hex".into());
        }
        Ok(())
    }
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
    /// name → exact version → entry.
    pub packages: BTreeMap<String, BTreeMap<String, PackageEntry>>,
}

/// The manifest file inside every package (ADR 0157 §3).
pub const MANIFEST_FILE: &str = "qrtz.json";
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
            crate::qrtz_json::check_name(id).map_err(|e| bad("index", e.to_string()))?;
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

    pub fn is_revoked(&self, id: &str, version: &str) -> bool {
        self.revoked
            .iter()
            .any(|r| r.package == id && (r.versions.is_empty() || r.versions.iter().any(|v| v == version)))
    }

    /// The highest version of `id` that isn't revoked.
    pub fn latest(&self, id: &str) -> Option<(&str, &PackageEntry)> {
        self.packages
            .get(id)?
            .iter()
            .filter(|(v, _)| !self.is_revoked(id, v))
            .max_by_key(|(v, _)| semver::Version::parse(v).ok())
            .map(|(v, e)| (v.as_str(), e))
    }

    /// Every unrevoked version of `id` matching `req`, highest first.
    pub fn matching(&self, id: &str, req: &semver::VersionReq) -> Vec<(semver::Version, &PackageEntry)> {
        let mut v: Vec<_> = self
            .packages
            .get(id)
            .into_iter()
            .flatten()
            .filter(|(v, _)| !self.is_revoked(id, v))
            .filter_map(|(v, e)| semver::Version::parse(v).ok().map(|s| (s, e)))
            .filter(|(s, _)| req.matches(s))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v
    }
}

impl PackageEntry {
    pub fn tree(&self) -> Tree {
        Tree {
            files: self.files.clone(),
        }
    }

    pub fn validate(&self, name: &str) -> Result<(), String> {
        let want = crate::qrtz_json::kind_of(name).map_err(|e| e.to_string())?;
        if want != self.kind {
            return Err(format!("kind {:?} doesn't match the name", self.kind));
        }
        if !crate::manifest::is_team_id(&self.team_id) || !crate::manifest::is_sha256(&self.sha256) {
            return Err("teamId or sha256 malformed".into());
        }
        if self.platform != PLATFORM {
            return Err(format!("platform must be {PLATFORM}"));
        }
        let tree = self.tree();
        tree.validate()?;
        if tree.size() != self.size {
            return Err("size must be the sum of the files".into());
        }
        if tree.sha256() != self.sha256 {
            return Err("sha256 must be the sha256 of the tree".into());
        }
        if !self
            .files
            .iter()
            .any(|f| f.path == MANIFEST_FILE && f.sha256 == self.manifest_sha256)
        {
            return Err("manifestSha256 must be the qrtz.json in files".into());
        }
        crate::qrtz_json::check_dependencies(name, self.kind, &self.dependencies)
            .map_err(|e| e.to_string())?;
        // ADR 0171: a model's files (all but its qrtz.json) come only from Hugging Face at a pinned commit; nothing
        // else names a URL (code and manifests come from modules.qrtz.ai).
        for f in &self.files {
            let weights = self.kind == PackageKind::Model && f.path != MANIFEST_FILE;
            if weights && f.mirrors.is_empty() {
                return Err(format!(
                    "{}: a model file needs its pinned Hugging Face URL",
                    f.path
                ));
            }
            if !weights && !f.mirrors.is_empty() {
                return Err(format!("{}: only a model's files have URLs", f.path));
            }
            if let Some(u) = f.mirrors.iter().find(|u| !is_pinned_hugging_face_url(u)) {
                return Err(format!(
                    "{}: {u:?} isn't a Hugging Face URL at a pinned commit",
                    f.path
                ));
            }
        }
        let code = self.kind.has_code();
        if code != self.identifier.is_some() || code != self.designated_requirement.is_some() {
            return Err("identifier and designatedRequirement only (and always) on code".into());
        }
        // ADR 0170: code is published only once Apple notarized exactly one of its files.
        match (&self.notarization, code) {
            (None, true) => return Err("notarization: code must carry its notarization (ADR 0170)".into()),
            (Some(_), false) => return Err("notarization only on code".into()),
            (Some(n), true) => {
                n.check()?;
                if !self
                    .files
                    .iter()
                    .any(|f| f.sha256 == n.code_sha256 && f.executable)
                {
                    return Err("notarization.codeSha256 must be one of the package's executables".into());
                }
            }
            (None, false) => {}
        }
        if code {
            let id = self.identifier.as_deref().unwrap_or_default();
            if Some(id.to_owned()) != crate::qrtz_json::identifier_for(name, self.kind)
                || self.designated_requirement.as_deref()
                    != Some(&crate::manifest::designated_requirement(id, &self.team_id))
            {
                return Err(
                    "identity must be the Developer ID requirement for the package's identifier".into(),
                );
            }
        }
        if (self.kind == PackageKind::Model) != self.task.is_some() {
            return Err("task only (and always) on models".into());
        }
        match self.kind {
            PackageKind::Service => {}
            // An extension's `models` is its `extension.reads` (the qrtz.json is checked against it at stage): each
            // one of its model dependencies (ADR 0212; the resolver derives grants from it).
            PackageKind::Extension => {
                for m in &self.models {
                    if !self.dependencies.contains_key(m) {
                        return Err(format!("models: {m} must be one of its dependencies"));
                    }
                }
            }
            _ if !self.models.is_empty() => return Err("models is for services and extension.reads".into()),
            _ => {}
        }
        for m in &self.models {
            if crate::qrtz_json::kind_of(m).ok() != Some(PackageKind::Model) {
                return Err(format!("models: {m} is not a model"));
            }
        }
        Ok(())
    }

    /// Every blob this package needs: its tree and each distinct file.
    pub fn blobs(&self) -> Vec<BlobRef> {
        let tree_bytes = self.tree().bytes();
        let mut v = vec![BlobRef {
            sha256: self.sha256.clone(),
            size: tree_bytes.len() as u64,
            mirrors: vec![],
        }];
        v.extend(self.tree().blobs());
        v
    }
}
