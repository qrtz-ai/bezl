//! Test support: a signing toolkit and a tiny HTTP/1.1 bundle server with fault injection (no external deps).
#![allow(dead_code)]
use ed25519_dalek::{Signer, SigningKey};
use qrtz_speech::bundle::PinnedKey;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const APP: &str = "0.5.0";
pub const FAR: u64 = 4_000_000_000_000;
pub const NOW: u64 = 1_800_000_000_000;

pub fn key(seed: u8, id: &str) -> (SigningKey, PinnedKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let pk = PinnedKey::from_hex(id, &hex::encode(sk.verifying_key().to_bytes())).unwrap();
    (sk, pk)
}
pub fn sign(sk: &SigningKey, b: &[u8]) -> Vec<u8> {
    hex::encode(sk.sign(b).to_bytes()).into_bytes()
}
pub fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub struct Bundle {
    pub version: String,
    pub manifest: Vec<u8>,
    pub sig: Vec<u8>,
    pub files: Vec<(String, Vec<u8>)>,
}

pub fn bundle(sk: &SigningKey, key_id: &str, version: &str, big: usize) -> Bundle {
    let files = vec![
        (
            "bin/qrtz-speech".to_string(),
            format!("#!module {version}\n").into_bytes(),
        ),
        ("health/golden.pcm".to_string(), vec![1u8; 3200]),
        (
            "engine/weights.bin".to_string(),
            (0..big).map(|i| (i % 251) as u8).collect(),
        ),
    ];
    let m = serde_json::json!({
        "module": "qrtz-speech", "version": version, "protocol": 1, "minApp": "0.1.0",
        "entry": "bin/qrtz-speech", "engine": {"id": "opaque-engine", "rev": version},
        "files": files.iter().map(|(p, b)| serde_json::json!({"path": p, "size": b.len(), "sha256": sha(b)})).collect::<Vec<_>>(),
        "health": {"clip": "health/golden.pcm", "expected": "the quick brown fox"},
        "keyId": key_id,
    });
    let manifest = serde_json::to_vec(&m).unwrap();
    let sig = sign(sk, &manifest);
    Bundle {
        version: version.into(),
        manifest,
        sig,
        files,
    }
}

pub const DAY: u64 = 24 * 60 * 60 * 1000;

/// Issued at min(NOW, expires - 7 days).
pub fn denylist(
    sk: &SigningKey,
    key_id: &str,
    seq: u64,
    expires: u64,
    versions: &[&str],
) -> (Vec<u8>, Vec<u8>) {
    denylist_at(
        sk,
        key_id,
        seq,
        expires.saturating_sub(7 * DAY).min(NOW),
        expires,
        versions,
    )
}

pub fn denylist_at(
    sk: &SigningKey,
    key_id: &str,
    seq: u64,
    issued: u64,
    expires: u64,
    versions: &[&str],
) -> (Vec<u8>, Vec<u8>) {
    let b = serde_json::to_vec(&serde_json::json!({
        "keyId": key_id, "seq": seq, "issued": issued, "expires": expires, "versions": versions,
    }))
    .unwrap();
    let s = sign(sk, &b);
    (b, s)
}

#[derive(Default)]
pub struct Faults {
    /// Paths answered 500 once.
    pub fail_once: HashSet<String>,
    /// Paths whose body is cut after n bytes once.
    pub truncate_once: HashMap<String, usize>,
    /// Paths served with flipped bytes.
    pub corrupt: HashSet<String>,
    /// Paths served with extra bytes.
    pub oversize: HashSet<String>,
    /// Range headers seen, per path.
    pub ranges: Vec<(String, String)>,
    pub hits: HashMap<String, usize>,
}

#[derive(Clone)]
pub struct Server {
    pub base: String,
    pub routes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pub faults: Arc<Mutex<Faults>>,
}

impl Server {
    pub async fn start() -> Self {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let s = Server {
            base,
            routes: Default::default(),
            faults: Default::default(),
        };
        let s2 = s.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else { return };
                let s = s2.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = c.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let req = String::from_utf8_lossy(&buf).to_string();
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .trim_start_matches('/')
                        .to_string();
                    let range = req.lines().find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("range").then(|| v.trim().to_string())
                    });
                    let body = s.routes.lock().unwrap().get(&path).cloned();
                    let (fail, cut, corrupt, over) = {
                        let mut f = s.faults.lock().unwrap();
                        *f.hits.entry(path.clone()).or_default() += 1;
                        if let Some(r) = &range {
                            f.ranges.push((path.clone(), r.clone()));
                        }
                        (
                            f.fail_once.remove(&path),
                            f.truncate_once.remove(&path),
                            f.corrupt.contains(&path),
                            f.oversize.contains(&path),
                        )
                    };
                    let Some(mut body) = body else {
                        let _ = c
                            .write_all(
                                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await;
                        return;
                    };
                    if fail {
                        let _ = c.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    }
                    if corrupt {
                        for b in body.iter_mut().step_by(97) {
                            *b ^= 0xff;
                        }
                    }
                    if over {
                        body.extend_from_slice(&[0u8; 1000]);
                    }
                    let (status, start) = match range
                        .as_deref()
                        .and_then(|r| r.strip_prefix("bytes="))
                        .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
                    {
                        Some(n) if n < body.len() => ("206 Partial Content", n),
                        _ => ("200 OK", 0),
                    };
                    let part = &body[start..];
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        part.len()
                    );
                    let _ = c.write_all(head.as_bytes()).await;
                    match cut {
                        Some(n) => {
                            let _ = c.write_all(&part[..n.min(part.len())]).await;
                        }
                        None => {
                            let _ = c.write_all(part).await;
                        }
                    }
                    let _ = c.shutdown().await;
                });
            }
        });
        s
    }

    /// Publish `b` as `latest` (and its files under `<version>/`).
    pub fn publish(&self, b: &Bundle) {
        let mut r = self.routes.lock().unwrap();
        r.insert("latest/manifest.json".into(), b.manifest.clone());
        r.insert("latest/manifest.sig".into(), b.sig.clone());
        for (p, bytes) in &b.files {
            r.insert(format!("{}/{}", b.version, p), bytes.clone());
        }
    }
    pub fn put(&self, path: &str, bytes: Vec<u8>) {
        self.routes.lock().unwrap().insert(path.into(), bytes);
    }
}
