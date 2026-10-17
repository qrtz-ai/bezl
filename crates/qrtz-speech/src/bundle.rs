//! Signed bundle metadata (ADR 0046 §4): the manifest, its Ed25519 signature, key rotation and the denylist.
//! Every signature is over the exact bytes served; the parsed form is only trusted after `verify_*`.
use crate::SpeechError;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// Hard cap for one bundle (all files).
pub const MAX_BUNDLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Manifest, signature, rotation and denylist documents are small.
pub const MAX_META_BYTES: usize = 256 * 1024;
pub const MAX_FILES: usize = 512;
pub const MODULE_NAME: &str = "qrtz-speech";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// A bundled file: 16 kHz mono PCM16LE golden clip.
    pub clip: String,
    /// Expected transcript of the clip (lowercase words).
    pub expected: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub module: String,
    pub version: String,
    pub protocol: u32,
    pub min_app: String,
    #[serde(default)]
    pub max_app: Option<String>,
    /// The module executable, one of `files`.
    pub entry: String,
    /// Opaque engine/model metadata, shown in settings only.
    #[serde(default)]
    pub engine: serde_json::Value,
    pub files: Vec<FileEntry>,
    pub health: Health,
    pub key_id: String,
}

impl Manifest {
    pub fn semver(&self) -> semver::Version {
        semver::Version::parse(&self.version).expect("validated")
    }
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// N60: a rotation names the next module signing key. Signed by the offline speech root key **and** cross-signed
/// by the new key (`RotationSigs`); a stolen signing key alone cannot rotate.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyRotation {
    pub seq: u64,
    /// ms since the epoch; an expired rotation is not applied.
    pub expires: u64,
    pub previous_key_id: String,
    pub key_id: String,
    /// Hex Ed25519 public key.
    pub public_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RotationSigs {
    /// Hex signature by the root key over the exact rotation bytes.
    pub root: String,
    /// Hex signature by the new key over the same bytes.
    pub new: String,
}

/// N60: monotonic `seq` (a replay changes nothing) and `expires` (re-signed daily; installs need a fresh one).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Denylist {
    pub key_id: String,
    pub seq: u64,
    pub expires: u64,
    pub versions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PinnedKey {
    pub key_id: String,
    pub key: [u8; 32],
}

impl PinnedKey {
    pub fn from_hex(key_id: &str, hex_key: &str) -> Result<Self, SpeechError> {
        let b = hex::decode(hex_key).map_err(|_| SpeechError::Signature("bad key hex".into()))?;
        let key: [u8; 32] = b
            .try_into()
            .map_err(|_| SpeechError::Signature("bad key length".into()))?;
        VerifyingKey::from_bytes(&key).map_err(|_| SpeechError::Signature("bad key".into()))?;
        Ok(Self {
            key_id: key_id.into(),
            key,
        })
    }
    pub fn hex(&self) -> String {
        hex::encode(self.key)
    }
}

/// Verify an Ed25519 signature (`sig` is 64 raw bytes or 128 hex chars) over `bytes` with `key`.
pub fn verify_sig(key: &PinnedKey, bytes: &[u8], sig: &[u8]) -> Result<(), SpeechError> {
    let raw = if sig.len() == 64 {
        sig.to_vec()
    } else {
        hex::decode(String::from_utf8_lossy(sig).trim())
            .map_err(|_| SpeechError::Signature("signature encoding".into()))?
    };
    let sig = Signature::from_slice(&raw).map_err(|_| SpeechError::Signature("signature length".into()))?;
    VerifyingKey::from_bytes(&key.key)
        .map_err(|_| SpeechError::Signature("bad key".into()))?
        .verify_strict(bytes, &sig)
        .map_err(|_| SpeechError::Signature("signature does not verify".into()))
}

fn check_len(bytes: &[u8]) -> Result<(), SpeechError> {
    if bytes.len() > MAX_META_BYTES {
        return Err(SpeechError::Invalid("metadata too large".into()));
    }
    Ok(())
}

/// A relative path of normal components only (no `..`, no root, no empty parts, no backslashes).
pub fn safe_rel_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 255
        && !p.contains('\\')
        && !p.contains('\0')
        && Path::new(p)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Signature first, then the content rules. `app_version` gates min/max app.
pub fn verify_manifest(
    key: &PinnedKey,
    bytes: &[u8],
    sig: &[u8],
    app_version: &semver::Version,
) -> Result<Manifest, SpeechError> {
    check_len(bytes)?;
    verify_sig(key, bytes, sig)?;
    let m: Manifest =
        serde_json::from_slice(bytes).map_err(|e| SpeechError::Invalid(format!("manifest: {e}")))?;
    if m.key_id != key.key_id {
        return Err(SpeechError::Signature(
            "manifest keyId is not the pinned key".into(),
        ));
    }
    if m.module != MODULE_NAME {
        return Err(SpeechError::Invalid("not a qrtz-speech bundle".into()));
    }
    semver::Version::parse(&m.version).map_err(|_| SpeechError::Invalid("version".into()))?;
    if m.protocol != crate::protocol::SPEECH_PROTOCOL_VERSION {
        return Err(SpeechError::Incompatible(format!(
            "speech protocol {}",
            m.protocol
        )));
    }
    let min = semver::Version::parse(&m.min_app).map_err(|_| SpeechError::Invalid("minApp".into()))?;
    if app_version < &min {
        return Err(SpeechError::Incompatible(format!("needs app {min}")));
    }
    if let Some(max) = &m.max_app {
        let max = semver::Version::parse(max).map_err(|_| SpeechError::Invalid("maxApp".into()))?;
        if app_version > &max {
            return Err(SpeechError::Incompatible(format!("needs app <= {max}")));
        }
    }
    if m.files.is_empty() || m.files.len() > MAX_FILES {
        return Err(SpeechError::Invalid("file count".into()));
    }
    let mut seen = std::collections::BTreeSet::new();
    for f in &m.files {
        if !safe_rel_path(&f.path) || !seen.insert(f.path.as_str()) {
            return Err(SpeechError::Invalid(format!("file path {:?}", f.path)));
        }
        if f.sha256.len() != 64 || hex::decode(&f.sha256).is_err() {
            return Err(SpeechError::Invalid(format!("sha256 of {}", f.path)));
        }
        if f.path == "manifest.json" || f.path == "manifest.sig" {
            return Err(SpeechError::Invalid("reserved file name".into()));
        }
    }
    if m.total_size() > MAX_BUNDLE_BYTES {
        return Err(SpeechError::TooLarge);
    }
    if m.file(&m.entry).is_none() || m.file(&m.health.clip).is_none() {
        return Err(SpeechError::Invalid("entry/health clip not in files".into()));
    }
    Ok(m)
}

/// Root signature + the new key's cross-signature, chaining from the pinned key, newer `seq`, not expired.
pub fn verify_rotation(
    root: &PinnedKey,
    pinned: &PinnedKey,
    last_seq: u64,
    now_ms: u64,
    bytes: &[u8],
    sigs: &[u8],
) -> Result<(PinnedKey, KeyRotation), SpeechError> {
    check_len(bytes)?;
    check_len(sigs)?;
    let sigs: RotationSigs =
        serde_json::from_slice(sigs).map_err(|_| SpeechError::Signature("rotation signatures".into()))?;
    verify_sig(root, bytes, sigs.root.as_bytes())?;
    let r: KeyRotation =
        serde_json::from_slice(bytes).map_err(|e| SpeechError::Invalid(format!("rotation: {e}")))?;
    if r.previous_key_id != pinned.key_id || r.key_id == pinned.key_id {
        return Err(SpeechError::Signature(
            "rotation does not chain from the pinned key".into(),
        ));
    }
    if r.seq <= last_seq {
        return Err(SpeechError::Signature("rotation replayed (seq)".into()));
    }
    if r.expires <= now_ms {
        return Err(SpeechError::Signature("rotation expired".into()));
    }
    let new = PinnedKey::from_hex(&r.key_id, &r.public_key)?;
    verify_sig(&new, bytes, sigs.new.as_bytes())?;
    Ok((new, r))
}

pub fn verify_denylist(key: &PinnedKey, bytes: &[u8], sig: &[u8]) -> Result<Denylist, SpeechError> {
    check_len(bytes)?;
    verify_sig(key, bytes, sig)?;
    let d: Denylist =
        serde_json::from_slice(bytes).map_err(|e| SpeechError::Invalid(format!("denylist: {e}")))?;
    if d.key_id != key.key_id {
        return Err(SpeechError::Signature(
            "denylist keyId is not the pinned key".into(),
        ));
    }
    Ok(d)
}
