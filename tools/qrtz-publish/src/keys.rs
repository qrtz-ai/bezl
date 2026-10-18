//! Signing keys, from a key dir or the environment. A key is a 32-byte Ed25519 seed in hex. Nothing here prints a
//! secret: `SigningKey`s are never formatted, and errors name files and variables, not contents.
//!
//! Key dir: `root-<name>.key` (one per root key; the offline ones), `timestamp.key`, `index.key`, each 0600, and
//! the public halves `<role>.pub` (so a machine that holds only the index key can still name the root keys).
//! Env: `QRTZ_PUBLISH_ROOT_KEYS` (comma-separated), `QRTZ_PUBLISH_TIMESTAMP_KEY`, `QRTZ_PUBLISH_INDEX_KEY`.
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use qrtz_modules::index::{canonical, key_id, Signature, Signatures};
use std::path::Path;

pub const ENV_ROOT: &str = "QRTZ_PUBLISH_ROOT_KEYS";
pub const ENV_TIMESTAMP: &str = "QRTZ_PUBLISH_TIMESTAMP_KEY";
pub const ENV_INDEX: &str = "QRTZ_PUBLISH_INDEX_KEY";

#[derive(Default)]
pub struct Keys {
    pub root: Vec<SigningKey>,
    /// Root public keys whose private half isn't here (the other offline key).
    pub root_public: Vec<VerifyingKey>,
    pub timestamp: Option<SigningKey>,
    pub timestamp_public: Option<VerifyingKey>,
    pub index: Option<SigningKey>,
    pub index_public: Option<VerifyingKey>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Keys {{ root: {}, timestamp: {}, index: {} }}",
            self.root.len(),
            self.timestamp.is_some(),
            self.index.is_some()
        )
    }
}

fn parse_seed(what: &str, s: &str) -> Result<SigningKey, String> {
    let b = hex::decode(s.trim()).map_err(|_| format!("{what}: not hex"))?;
    let b: [u8; 32] = b.try_into().map_err(|_| format!("{what}: not 32 bytes"))?;
    Ok(SigningKey::from_bytes(&b))
}

fn parse_pub(what: &str, s: &str) -> Result<VerifyingKey, String> {
    let b = hex::decode(s.trim()).map_err(|_| format!("{what}: not hex"))?;
    let b: [u8; 32] = b.try_into().map_err(|_| format!("{what}: not 32 bytes"))?;
    VerifyingKey::from_bytes(&b).map_err(|_| format!("{what}: not an ed25519 key"))
}

fn read_secret(p: &Path) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        if m.permissions().mode() & 0o077 != 0 {
            return Err(format!("{}: must be 0600 (chmod 600)", p.display()));
        }
    }
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

impl Keys {
    pub fn generate() -> SigningKey {
        use rand::RngCore;
        let mut b = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut b);
        SigningKey::from_bytes(&b)
    }

    /// Test keys made at runtime (dry-run): two root keys, a timestamp and an index key. Held in memory only.
    pub fn ephemeral() -> Self {
        let ts = Self::generate();
        let ix = Self::generate();
        Self {
            root: vec![Self::generate(), Self::generate()],
            root_public: vec![],
            timestamp_public: Some(ts.verifying_key()),
            timestamp: Some(ts),
            index_public: Some(ix.verifying_key()),
            index: Some(ix),
        }
    }

    pub fn from_dir(dir: &Path) -> Result<Self, String> {
        let mut k = Self::default();
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in &names {
            let p = dir.join(n);
            match n.as_str() {
                "timestamp.key" => k.timestamp = Some(parse_seed(n, &read_secret(&p)?)?),
                "index.key" => k.index = Some(parse_seed(n, &read_secret(&p)?)?),
                "timestamp.pub" => {
                    k.timestamp_public = Some(parse_pub(
                        n,
                        &std::fs::read_to_string(&p).map_err(|e| e.to_string())?,
                    )?)
                }
                "index.pub" => {
                    k.index_public = Some(parse_pub(
                        n,
                        &std::fs::read_to_string(&p).map_err(|e| e.to_string())?,
                    )?)
                }
                _ if n.starts_with("root-") && n.ends_with(".key") => {
                    k.root.push(parse_seed(n, &read_secret(&p)?)?)
                }
                _ if n.starts_with("root-") && n.ends_with(".pub") => {
                    let v = parse_pub(n, &std::fs::read_to_string(&p).map_err(|e| e.to_string())?)?;
                    k.root_public.push(v)
                }
                _ => {}
            }
        }
        k.fill_publics();
        Ok(k)
    }

    pub fn from_env() -> Result<Self, String> {
        let mut k = Self::default();
        if let Ok(v) = std::env::var(ENV_ROOT) {
            for (i, s) in v.split(',').filter(|s| !s.trim().is_empty()).enumerate() {
                k.root.push(parse_seed(&format!("{ENV_ROOT}[{i}]"), s)?);
            }
        }
        if let Ok(v) = std::env::var(ENV_TIMESTAMP) {
            k.timestamp = Some(parse_seed(ENV_TIMESTAMP, &v)?);
        }
        if let Ok(v) = std::env::var(ENV_INDEX) {
            k.index = Some(parse_seed(ENV_INDEX, &v)?);
        }
        k.fill_publics();
        Ok(k)
    }

    fn fill_publics(&mut self) {
        if let Some(t) = &self.timestamp {
            self.timestamp_public.get_or_insert(t.verifying_key());
        }
        if let Some(i) = &self.index {
            self.index_public.get_or_insert(i.verifying_key());
        }
        let have: Vec<_> = self.root.iter().map(|k| k.verifying_key()).collect();
        self.root_public.retain(|p| !have.contains(p));
    }

    /// Every root public key: the private ones here plus the `.pub`-only ones.
    pub fn root_keys(&self) -> Vec<VerifyingKey> {
        let mut v: Vec<_> = self.root.iter().map(|k| k.verifying_key()).collect();
        v.extend(self.root_public.iter().copied());
        v.sort_by_key(|k| key_id(&k.to_bytes()));
        v.dedup();
        v
    }
}

/// A `.sig` file: each key's Ed25519 signature over the exact bytes, sorted by key id.
pub fn sign(bytes: &[u8], keys: &[&SigningKey]) -> Vec<u8> {
    let mut signatures: Vec<Signature> = keys
        .iter()
        .map(|k| Signature {
            keyid: key_id(&k.verifying_key().to_bytes()),
            sig: hex::encode(k.sign(bytes).to_bytes()),
        })
        .collect();
    signatures.sort_by(|a, b| a.keyid.cmp(&b.keyid));
    signatures.dedup_by(|a, b| a.keyid == b.keyid);
    canonical(&Signatures { signatures })
}

/// `keygen`: write a fresh seed (0600) and its public half. Refuses to overwrite.
pub fn write_keypair(dir: &Path, role: &str) -> Result<VerifyingKey, String> {
    use std::io::Write;
    let key = dir.join(format!("{role}.key"));
    let public = dir.join(format!("{role}.pub"));
    if key.exists() || public.exists() {
        return Err(format!("{}: exists; refusing to overwrite a key", key.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let k = Keys::generate();
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(&key).map_err(|e| format!("{}: {e}", key.display()))?;
    f.write_all(hex::encode(k.to_bytes()).as_bytes())
        .map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    std::fs::write(&public, hex::encode(k.verifying_key().to_bytes())).map_err(|e| e.to_string())?;
    Ok(k.verifying_key())
}
