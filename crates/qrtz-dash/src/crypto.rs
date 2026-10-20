//! HMAC-SHA256 token hashing (spec: fast per-request check with a server pepper) and random secrets.
use rand::TryRngCore;
use sha2::{Digest, Sha256};

/// The server pepper. Never serialized, logged or shown (Debug is redacted).
#[derive(Clone)]
pub struct Pepper([u8; 32]);

impl Pepper {
    pub fn random() -> Self {
        let mut k = [0u8; 32];
        os_random(&mut k);
        Pepper(k)
    }
    pub fn from_bytes(k: [u8; 32]) -> Self {
        Pepper(k)
    }
    pub(crate) fn hash(&self, secret: &str) -> String {
        hex::encode(hmac_sha256(&self.0, secret.as_bytes()))
    }
}

impl std::fmt::Debug for Pepper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pepper(<redacted>)")
    }
}

/// RFC 2104 HMAC with SHA-256.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// OS randomness only (rand 0.9's OsRng is fallible): no secret is ever made from anything else.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fail closed: no secret is ever made from anything but the OS random source"
)]
pub(crate) fn os_random(b: &mut [u8]) {
    rand::rngs::OsRng
        .try_fill_bytes(b)
        .expect("the OS random source failed");
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    os_random(&mut b);
    hex::encode(b)
}

/// Constant-time compare of two hex hashes.
pub(crate) fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    #[test]
    fn hmac_matches_rfc4231_case_2() {
        let mac = super::hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
