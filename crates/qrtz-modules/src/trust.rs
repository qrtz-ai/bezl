//! The client's trust state for the signed index (ADR 0153) and the checks that move it forward.
//!
//! State (`trust.json`, 0600, written through temp + rename): the newest verified root, the highest timestamp and
//! index versions seen (with their hashes, so a version can't be replayed with other content), the clock high-water
//! mark (ADR 0046 §4b: expiry is measured against `max(now, every issued seen)`, so winding the clock back doesn't
//! un-expire anything), and the revocation snapshot in force.
//!
//! - **Roots** chain from the root pinned in the build (its sha256): `N+1` must be signed by a threshold of `N`'s
//!   root keys and of its own, and have version exactly `N+1`. Only the newest must be unexpired, and only on the
//!   network path.
//! - **Timestamp** (`timestamp` role): never older than stored (`Rollback`), never the same version with other
//!   bytes (`Equivocation`). On the network path it must be unexpired (`Expired`): this is the freeze-attack check.
//! - **Index** (`index` role): its sha256 and size are the ones the timestamp names, its version is the one the
//!   timestamp names and never below stored.
//! - **Revocations** accumulate: an adopted list's entries are added to every list seen before (ADR 0046 §4b
//!   condition 1, the set never shrinks).
use crate::index::{layout, Index, IndexError, Root, Signatures, Timestamp, MAX_INDEX_BYTES, MAX_META_BYTES};
use crate::sideload::{RevocationSnapshot, RevocationVerifier, Revoked};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const TRUST_FILE: &str = "trust.json";
pub const TRUST_VERSION: u32 = 1;

/// Which freshness rule applies (Bozniak's sideload rule 1 vs 2; rule 7: nothing else differs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Network install, update or refresh: an expired timestamp or root is refused, no override.
    Network,
    /// A user-picked `.qrtzpkg`: expiry is reported, not refused; revocations and rollback still apply.
    Sideload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Seen {
    pub version: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustState {
    pub trust_version: u32,
    /// The newest verified root, exact bytes (hex would double it; the doc is canonical json, kept as a string).
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<Seen>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<Seen>,
    pub clock_hwm_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation: Option<RevocationSnapshot>,
}

/// A verified pair: the timestamp and the index it names. The only way to get one is [`Trust::verify_pair`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub timestamp: Timestamp,
    pub timestamp_sha256: String,
    pub index: Index,
    pub index_bytes: Vec<u8>,
    pub index_sig: Vec<u8>,
}

impl Verified {
    /// The revocation view of this pair: versioned and dated by the timestamp, listing the index's revocations.
    pub fn snapshot(&self) -> RevocationSnapshot {
        RevocationSnapshot {
            version: self.timestamp.version,
            issued_ms: self.timestamp.issued_ms,
            expires_ms: self.timestamp.expires_ms,
            revoked: self.index.revoked.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trust {
    pub state: TrustState,
    pub root: Root,
}

/// Merge `newer`'s entries into `older`'s: what was ever revoked stays revoked.
pub fn accumulate(older: Option<&RevocationSnapshot>, newer: &RevocationSnapshot) -> RevocationSnapshot {
    let mut out = newer.clone();
    if let Some(o) = older {
        for r in &o.revoked {
            if !out.revoked.contains(r) {
                out.revoked.push(r.clone());
            }
        }
    }
    out.revoked.sort_by(|a: &Revoked, b: &Revoked| {
        (&a.package, &a.versions, &a.reason).cmp(&(&b.package, &b.versions, &b.reason))
    });
    out
}

impl Trust {
    /// First run: `1.root.json` must hash to the build's pin and be signed by its own root role.
    pub fn bootstrap(
        pinned_root_sha256: Option<&str>,
        root_bytes: &[u8],
        root_sig: &[u8],
    ) -> Result<Self, IndexError> {
        let pin = pinned_root_sha256.ok_or(IndexError::NoPinnedRoot)?;
        if crate::sha256_hex(root_bytes) != pin {
            return Err(IndexError::UnpinnedRoot);
        }
        let root = Root::parse(root_bytes)?;
        if root.version != 1 {
            return Err(IndexError::Invalid("root", "the pinned root is version 1".into()));
        }
        root.verify_role(
            "root",
            &root.roles.root,
            root_bytes,
            &Signatures::parse(root_sig)?,
        )?;
        Ok(Self {
            state: TrustState {
                trust_version: TRUST_VERSION,
                root: String::from_utf8(root_bytes.to_vec())
                    .map_err(|e| IndexError::Invalid("root", e.to_string()))?,
                timestamp: None,
                index: None,
                clock_hwm_ms: root.issued_ms,
                revocation: None,
            },
            root,
        })
    }

    pub fn load(path: &Path) -> Result<Option<Self>, IndexError> {
        let b = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(IndexError::Invalid("trust.json", e.to_string())),
        };
        let state: TrustState =
            serde_json::from_slice(&b).map_err(|e| IndexError::Invalid("trust.json", e.to_string()))?;
        if state.trust_version != TRUST_VERSION {
            return Err(IndexError::Invalid("trust.json", "unsupported version".into()));
        }
        let root = Root::parse(state.root.as_bytes())?;
        Ok(Some(Self { state, root }))
    }

    pub fn save(&self, path: &Path) -> Result<(), crate::ModuleError> {
        let bytes =
            serde_json::to_vec_pretty(&self.state).map_err(|e| crate::ModuleError::Json(e.to_string()))?;
        crate::fsutil::write_private_atomic(path, &bytes)
    }

    /// `max(now, every issued time seen)`: what expiry is measured against.
    pub fn effective_now(&self, now_ms: u64) -> u64 {
        now_ms.max(self.state.clock_hwm_ms)
    }

    pub fn note_clock(&mut self, now_ms: u64) {
        self.state.clock_hwm_ms = self.state.clock_hwm_ms.max(now_ms);
    }

    /// Root `N+1`. Signed by the current root's root role and its own.
    pub fn update_root(&mut self, bytes: &[u8], sig: &[u8]) -> Result<(), IndexError> {
        let next = Root::parse(bytes)?;
        let sigs = Signatures::parse(sig)?;
        if next.version != self.root.version + 1 {
            return Err(IndexError::Invalid(
                "root",
                format!("expected version {}", self.root.version + 1),
            ));
        }
        self.root
            .verify_role("root (old keys)", &self.root.roles.root, bytes, &sigs)?;
        next.verify_role("root (new keys)", &next.roles.root, bytes, &sigs)?;
        let text =
            String::from_utf8(bytes.to_vec()).map_err(|e| IndexError::Invalid("root", e.to_string()))?;
        self.root = next;
        self.state.root = text;
        // A key rotation of the timestamp or index role resets nothing: versions stay monotonic across roots.
        Ok(())
    }

    /// Network path only: the newest root must not have expired.
    pub fn check_root_fresh(&self, now_ms: u64) -> Result<(), IndexError> {
        if self.effective_now(now_ms) >= self.root.expires_ms {
            return Err(IndexError::Expired { what: "root" });
        }
        Ok(())
    }

    /// Verify a timestamp and the index it names. Nothing is stored; [`Trust::commit`] does that.
    pub fn verify_pair(
        &self,
        mode: Mode,
        ts_bytes: &[u8],
        ts_sig: &[u8],
        index_bytes: &[u8],
        index_sig: &[u8],
        now_ms: u64,
    ) -> Result<Verified, IndexError> {
        let timestamp = self.verify_timestamp(mode, ts_bytes, ts_sig, now_ms)?;
        self.verify_index_for(
            mode,
            &timestamp,
            crate::sha256_hex(ts_bytes),
            index_bytes,
            index_sig,
            now_ms,
        )
    }

    pub fn verify_timestamp(
        &self,
        mode: Mode,
        bytes: &[u8],
        sig: &[u8],
        now_ms: u64,
    ) -> Result<Timestamp, IndexError> {
        if bytes.len() > MAX_META_BYTES {
            return Err(IndexError::TooLarge("timestamp"));
        }
        let t = Timestamp::parse(bytes)?;
        self.root.verify_role(
            "timestamp",
            &self.root.roles.timestamp,
            bytes,
            &Signatures::parse(sig)?,
        )?;
        if mode == Mode::Network {
            if let Some(s) = &self.state.timestamp {
                if t.version < s.version {
                    return Err(IndexError::Rollback {
                        what: "timestamp",
                        got: t.version,
                        stored: s.version,
                    });
                }
                if t.version == s.version && crate::sha256_hex(bytes) != s.sha256 {
                    return Err(IndexError::Equivocation {
                        what: "timestamp",
                        version: t.version,
                    });
                }
            }
            if self.effective_now(now_ms) >= t.expires_ms {
                return Err(IndexError::Expired { what: "timestamp" });
            }
            self.check_root_fresh(now_ms)?;
        }
        Ok(t)
    }

    fn verify_index_for(
        &self,
        mode: Mode,
        t: &Timestamp,
        ts_sha: String,
        bytes: &[u8],
        sig: &[u8],
        now_ms: u64,
    ) -> Result<Verified, IndexError> {
        if bytes.len() > MAX_INDEX_BYTES {
            return Err(IndexError::TooLarge("index"));
        }
        if bytes.len() as u64 != t.index.size || crate::sha256_hex(bytes) != t.index.sha256 {
            return Err(IndexError::TimestampMismatch("index"));
        }
        self.root
            .verify_role("index", &self.root.roles.index, bytes, &Signatures::parse(sig)?)?;
        let index = Index::parse(bytes)?;
        if index.version != t.index.version {
            return Err(IndexError::TimestampMismatch("index version"));
        }
        if mode == Mode::Network {
            if let Some(s) = &self.state.index {
                if index.version < s.version {
                    return Err(IndexError::Rollback {
                        what: "index",
                        got: index.version,
                        stored: s.version,
                    });
                }
                if index.version == s.version && t.index.sha256 != s.sha256 {
                    return Err(IndexError::Equivocation {
                        what: "index",
                        version: index.version,
                    });
                }
            }
            if self.effective_now(now_ms) >= index.expires_ms {
                return Err(IndexError::Expired { what: "index" });
            }
        }
        Ok(Verified {
            timestamp: t.clone(),
            timestamp_sha256: ts_sha,
            index,
            index_bytes: bytes.to_vec(),
            index_sig: sig.to_vec(),
        })
    }

    /// Store a verified pair as the newest state (network refresh, or a carried pair that was adopted). Refuses a
    /// pair older than what is stored, so a caller can't move the state backwards.
    pub fn commit(&mut self, v: &Verified, now_ms: u64) -> Result<(), IndexError> {
        if let Some(s) = &self.state.timestamp {
            if v.timestamp.version < s.version {
                return Err(IndexError::Rollback {
                    what: "timestamp",
                    got: v.timestamp.version,
                    stored: s.version,
                });
            }
        }
        if let Some(s) = &self.state.index {
            if v.index.version < s.version {
                return Err(IndexError::Rollback {
                    what: "index",
                    got: v.index.version,
                    stored: s.version,
                });
            }
        }
        self.state.timestamp = Some(Seen {
            version: v.timestamp.version,
            sha256: v.timestamp_sha256.clone(),
        });
        self.state.index = Some(Seen {
            version: v.index.version,
            sha256: v.timestamp.index.sha256.clone(),
        });
        self.state.revocation = Some(accumulate(self.state.revocation.as_ref(), &v.snapshot()));
        self.note_clock(now_ms);
        self.note_clock(v.timestamp.issued_ms);
        self.note_clock(v.index.issued_ms);
        Ok(())
    }

    /// A verifier for a carried index, bound to a carried timestamp already checked with [`Mode::Sideload`]
    /// (the `RevocationVerifier` that `sideload::VerifiedSnapshot::verify` needs).
    pub fn carried<'a>(&'a self, timestamp: &'a Timestamp, timestamp_sha256: String) -> CarriedVerifier<'a> {
        CarriedVerifier {
            trust: self,
            timestamp,
            timestamp_sha256,
        }
    }

    /// The revocation list stored, accumulated.
    pub fn revocation(&self) -> Option<&RevocationSnapshot> {
        self.state.revocation.as_ref()
    }

    /// The paths of every root newer than ours, in order, for a host to be asked for (the client stops at the
    /// first one the host doesn't have).
    pub fn next_root_path(&self) -> String {
        layout::root(self.root.version + 1)
    }
}

pub struct CarriedVerifier<'a> {
    trust: &'a Trust,
    timestamp: &'a Timestamp,
    timestamp_sha256: String,
}

impl RevocationVerifier for CarriedVerifier<'_> {
    fn verify(&self, bytes: &[u8], sig: &[u8]) -> Result<RevocationSnapshot, String> {
        self.trust
            .verify_index_for(
                Mode::Sideload,
                self.timestamp,
                self.timestamp_sha256.clone(),
                bytes,
                sig,
                0,
            )
            .map(|v| accumulate(self.trust.revocation(), &v.snapshot()))
            .map_err(|e| e.to_string())
    }
}
