//! Resumable, size-limited bundle download (ADR 0046 §4). HTTPS is transport only: nothing fetched is trusted
//! before the manifest signature and the per-file sha256 check.
//!
//! Server layout under `base`: `latest/manifest.json` + `.sig`, files at `<version>/<path>`, optional
//! `denylist.json` + `denylist.json.sig` (required for installs, N60) and `key-rotation.json` +
//! `key-rotation.sigs.json`.
use crate::bundle::{FileEntry, Manifest, MAX_META_BYTES};
use crate::fsx;
use crate::SpeechError;
use futures_util::StreamExt;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// Pause = stop the transfer and keep the `.part` files; calling `download_bundle` again resumes.
#[derive(Clone, Default)]
pub struct DownloadControl {
    paused: Arc<AtomicBool>,
}

impl DownloadControl {
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

/// N100: true for an http(s) URL whose host really is loopback (`localhost`, `127.0.0.0/8`, `::1`), so
/// `http://127.0.0.1.example.com` is not (qrtz-app's `the_loopback_check_is_strict`).
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    match u.host_str() {
        Some(h) if h.eq_ignore_ascii_case("localhost") => true,
        Some(h) => h
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|a| a.is_loopback()),
        None => false,
    }
}

/// QRT-313: the most redirects [`Downloader::fetch_following`] follows (GitHub uses one or two).
pub const MAX_HOPS: usize = 5;

/// Why [`Downloader::fetch_following`] stopped.
#[derive(Debug)]
pub enum FollowError {
    /// The caller's `allow` refused this URL (a hop or the final answer); nothing was read from it.
    Blocked(String),
    Other(SpeechError),
}

pub struct Downloader {
    client: reqwest::Client,
    base: String,
}

impl Downloader {
    pub fn new(base: &str) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(60))
            // N100: plain http only to a real loopback host (not `http://127.0.0.1.example.com`).
            .https_only(!is_loopback_url(base))
            .build()
            .expect("http client");
        Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    /// A small signed document; `Ok(None)` on 404.
    pub async fn fetch_meta(&self, path: &str) -> Result<Option<Vec<u8>>, SpeechError> {
        let r = self
            .client
            .get(format!("{}/{path}", self.base))
            .send()
            .await
            .map_err(|e| SpeechError::Network(e.to_string()))?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err(SpeechError::Network(format!("{path}: http {}", r.status())));
        }
        if r.content_length().is_some_and(|n| n > MAX_META_BYTES as u64) {
            return Err(SpeechError::TooLarge);
        }
        let mut out = Vec::new();
        let mut s = r.bytes_stream();
        while let Some(c) = s.next().await {
            let c = c.map_err(|e| SpeechError::Network(e.to_string()))?;
            if out.len() + c.len() > MAX_META_BYTES {
                return Err(SpeechError::TooLarge);
            }
            out.extend_from_slice(&c);
        }
        Ok(Some(out))
    }

    /// Bytes `start..start+len` of `<base>/<path>` (ADR 0157's package fetcher: whole blobs in parallel ranges,
    /// resumed range by range). The host must answer 206 with exactly `len` bytes, or 200 with the whole body when
    /// that is exactly the range asked for; anything longer is cut off and refused. The caller checks the sha256.
    /// A pause stops it between body chunks (`Paused`; the range is fetched again on resume).
    pub async fn fetch_range(
        &self,
        path: &str,
        start: u64,
        len: u64,
        ctl: &DownloadControl,
    ) -> Result<Vec<u8>, SpeechError> {
        if len == 0 {
            return Ok(vec![]);
        }
        let r = self
            .client
            .get(format!("{}/{path}", self.base))
            .header(
                reqwest::header::RANGE,
                format!("bytes={start}-{}", start + len - 1),
            )
            .send()
            .await
            .map_err(|e| SpeechError::Network(e.to_string()))?;
        let st = r.status();
        let whole = st == reqwest::StatusCode::OK && start == 0;
        if st != reqwest::StatusCode::PARTIAL_CONTENT && !whole {
            return Err(SpeechError::Network(format!("{path}: http {st} for a range")));
        }
        if r.content_length().is_some_and(|n| n != len) {
            return Err(SpeechError::Network(format!("{path}: range of the wrong length")));
        }
        let mut out = Vec::with_capacity(len.min(16 << 20) as usize);
        let mut s = r.bytes_stream();
        while let Some(c) = s.next().await {
            if ctl.is_paused() {
                return Err(SpeechError::Paused);
            }
            let c = c.map_err(|e| SpeechError::Network(e.to_string()))?;
            if (out.len() + c.len()) as u64 > len {
                return Err(SpeechError::TooLarge);
            }
            out.extend_from_slice(&c);
        }
        if out.len() as u64 != len {
            return Err(SpeechError::Network(format!("{path}: short range")));
        }
        Ok(out)
    }

    /// QRT-313: a client that follows no redirect by itself, for [`Downloader::fetch_following`] (the caller's
    /// `allow` judges every hop). HTTPS only.
    pub fn no_redirects() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(60))
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client");
        Self {
            client,
            base: String::new(),
        }
    }

    /// QRT-313: GET the absolute `url`, following at most [`MAX_HOPS`] redirects by hand. `allow(url, last)` is
    /// asked about every URL before it is requested (`last == false`) and again about the one that finally answers
    /// (`last == true`); a `false` stops at once with [`FollowError::Blocked`] naming that URL, before any byte of
    /// it is read. The body (at most `max` bytes, else refused) goes to `sink` chunk by chunk. `Ok(false)` = 404.
    pub async fn fetch_following(
        &self,
        url: &str,
        allow: &(dyn Fn(&str, bool) -> bool + Send + Sync),
        max: u64,
        sink: &mut (dyn FnMut(&[u8]) -> Result<(), String> + Send),
    ) -> Result<bool, FollowError> {
        let net = |e: String| FollowError::Other(SpeechError::Network(e));
        let mut cur = reqwest::Url::parse(url).map_err(|e| net(format!("{url}: {e}")))?;
        for _ in 0..=MAX_HOPS {
            if !allow(cur.as_str(), false) {
                return Err(FollowError::Blocked(cur.to_string()));
            }
            let r = self
                .client
                .get(cur.clone())
                .send()
                .await
                .map_err(|e| net(e.to_string()))?;
            let st = r.status();
            if st.is_redirection() {
                let loc = r
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| net(format!("{cur}: http {st} without a location")))?;
                cur = cur
                    .join(loc)
                    .map_err(|e| net(format!("{cur}: bad location: {e}")))?;
                continue;
            }
            if !allow(cur.as_str(), true) {
                return Err(FollowError::Blocked(cur.to_string()));
            }
            if st == reqwest::StatusCode::NOT_FOUND {
                return Ok(false);
            }
            if !st.is_success() {
                return Err(net(format!("{}: http {st}", cur.path())));
            }
            if r.content_length().is_some_and(|n| n > max) {
                return Err(FollowError::Other(SpeechError::TooLarge));
            }
            let mut done = 0u64;
            let mut s = r.bytes_stream();
            while let Some(c) = s.next().await {
                let c = c.map_err(|e| net(e.to_string()))?;
                done += c.len() as u64;
                if done > max {
                    return Err(FollowError::Other(SpeechError::TooLarge));
                }
                sink(&c).map_err(net)?;
            }
            return Ok(true);
        }
        Err(net(format!("{url}: more than {MAX_HOPS} redirects")))
    }

    /// Download every file of `m` into `staging` (resuming `.part` files, skipping finished ones). Each file is
    /// capped at its manifest size and checked against its sha256 before it is renamed into place.
    pub async fn download_bundle(
        &self,
        m: &Manifest,
        staging: &Path,
        ctl: &DownloadControl,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<(), SpeechError> {
        let files: Vec<(String, FileEntry)> = m
            .files
            .iter()
            .map(|f| (format!("{}/{}", m.version, f.path), f.clone()))
            .collect();
        self.download_files(&files, staging, ctl, on_progress).await
    }

    /// The same resumable, capped, sha256-checked transfer for any pinned list: each `(url_path, entry)` is fetched
    /// from `<base>/<url_path>` into `<dest>/<entry.path>` (ADR 0079: the Mac helper's local model files, whose
    /// sha256s are pinned in the app, use this with the model host as `base`).
    pub async fn download_files(
        &self,
        files: &[(String, FileEntry)],
        staging: &Path,
        ctl: &DownloadControl,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<(), SpeechError> {
        let total: u64 = files.iter().map(|(_, f)| f.size).sum();
        let mut done_files = 0u64;
        for (url_path, f) in files {
            let dest = staging.join(&f.path);
            if let Some(parent) = dest.parent() {
                fsx::mkdir_private(parent)?;
            }
            if dest.exists() {
                let (n, h) = fsx::sha256_file(&dest)?;
                if n == f.size && h == f.sha256 {
                    done_files += f.size;
                    on_progress(Progress {
                        done: done_files,
                        total,
                    });
                    continue;
                }
                std::fs::remove_file(&dest)?;
            }
            let part = staging.join(format!("{}.part", f.path));
            let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if have > f.size {
                std::fs::remove_file(&part)?;
                have = 0;
            }
            if have < f.size {
                if ctl.is_paused() {
                    return Err(SpeechError::Paused);
                }
                let mut req = self.client.get(format!("{}/{url_path}", self.base));
                if have > 0 {
                    req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
                }
                let r = req
                    .send()
                    .await
                    .map_err(|e| SpeechError::Network(e.to_string()))?;
                let status = r.status();
                let mut file = if have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
                    fsx::open_private_append(&part)?
                } else if status.is_success() {
                    have = 0;
                    let _ = std::fs::remove_file(&part);
                    fsx::open_private_append(&part)?
                } else {
                    return Err(SpeechError::Network(format!("{}: http {status}", f.path)));
                };
                let mut s = r.bytes_stream();
                let mut last_report = 0u64;
                while let Some(c) = s.next().await {
                    if ctl.is_paused() {
                        file.flush()?;
                        return Err(SpeechError::Paused);
                    }
                    let c = c.map_err(|e| SpeechError::Network(e.to_string()))?;
                    if have + c.len() as u64 > f.size {
                        drop(file);
                        let _ = std::fs::remove_file(&part);
                        return Err(SpeechError::TooLarge);
                    }
                    file.write_all(&c)?;
                    have += c.len() as u64;
                    if have - last_report >= 256 * 1024 || have == f.size {
                        last_report = have;
                        on_progress(Progress {
                            done: done_files + have,
                            total,
                        });
                    }
                }
                file.sync_all()?;
                if have < f.size {
                    return Err(SpeechError::Network(format!(
                        "{}: connection closed early",
                        f.path
                    )));
                }
            }
            let (n, h) = fsx::sha256_file(&part)?;
            if n != f.size || h != f.sha256 {
                let _ = std::fs::remove_file(&part);
                return Err(SpeechError::Checksum(f.path.clone()));
            }
            std::fs::rename(&part, &dest)?;
            done_files += f.size;
            on_progress(Progress {
                done: done_files,
                total,
            });
        }
        Ok(())
    }
}
