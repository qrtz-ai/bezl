//! Test kit (feature `testkit`): a signing toolkit, signed bundles and denylists, and a tiny HTTP/1.1 bundle
//! server with fault injection. Used by this crate's tests and by qrtz-app's E2E tests (settings speech setup, attach).
use crate::bundle::PinnedKey;
use ed25519_dalek::{Signer, SigningKey};
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
    /// Paths served in 16 KiB chunks with this pause between them (pause/resume tests).
    pub throttle: HashMap<String, std::time::Duration>,
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
                    let (fail, cut, corrupt, over, slow) = {
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
                            f.throttle.get(&path).copied(),
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
                    // `bytes=N-` and `bytes=a-b` (closed, inclusive), answered 206 with a Content-Range.
                    let want = range
                        .as_deref()
                        .and_then(|r| r.strip_prefix("bytes="))
                        .and_then(|r| {
                            let (a, b) = r.split_once('-')?;
                            let a = a.parse::<usize>().ok()?;
                            let b = if b.is_empty() {
                                body.len().checked_sub(1)?
                            } else {
                                b.parse::<usize>().ok()?
                            };
                            (a <= b && a < body.len()).then_some((a, b.min(body.len() - 1)))
                        });
                    let (status, start, end) = match want {
                        Some((a, b)) => ("206 Partial Content", a, b + 1),
                        None => ("200 OK", 0, body.len()),
                    };
                    let part = &body[start..end];
                    let extra = if want.is_some() {
                        format!("Content-Range: bytes {start}-{}/{}\r\n", end - 1, body.len())
                    } else {
                        String::new()
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                        part.len()
                    );
                    let _ = c.write_all(head.as_bytes()).await;
                    match cut {
                        Some(n) => {
                            let _ = c.write_all(&part[..n.min(part.len())]).await;
                        }
                        None => match slow {
                            Some(gap) => {
                                for chunk in part.chunks(16 * 1024) {
                                    if c.write_all(chunk).await.is_err() {
                                        return;
                                    }
                                    tokio::time::sleep(gap).await;
                                }
                            }
                            None => {
                                let _ = c.write_all(part).await;
                            }
                        },
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

/// A bundle whose entry is the mock module binary and whose one model file is its JSON config.
/// A bundle whose entry is `entry` (the `qrtz-mock-speech` binary) configured by `cfg` (engine/mock.json).
pub fn mock_bundle_with(
    sk: &SigningKey,
    key_id: &str,
    version: &str,
    cfg: serde_json::Value,
    expected: &str,
    entry: Vec<u8>,
) -> Bundle {
    let files = vec![
        ("bin/qrtz-speech".to_string(), entry),
        ("health/golden.pcm".to_string(), vec![0u8; 3200]),
        ("engine/mock.json".to_string(), cfg.to_string().into_bytes()),
    ];
    let m = serde_json::json!({
        "module": "qrtz-speech", "version": version, "protocol": 1, "minApp": "0.1.0",
        "entry": "bin/qrtz-speech",
        "files": files.iter().map(|(p, b)| serde_json::json!({"path": p, "size": b.len(), "sha256": sha(b)})).collect::<Vec<_>>(),
        "health": {"clip": "health/golden.pcm", "expected": expected},
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
