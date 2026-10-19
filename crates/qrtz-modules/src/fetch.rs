//! The one fetcher (ADR 0157 §6): every blob, of every package kind, from every source, through here.
//!
//! - **Sources** ([`sources`]): a model file only from its pinned Hugging Face URLs in the signed tree (ADR 0171:
//!   model bytes are never on modules.qrtz.ai); every other blob from each module host (`modules.qrtz.ai`; ADR 0167:
//!   the sole package host) at `v1/blobs/sha256/<hex>`. A source that fails, or whose bytes don't hash right, is dropped for the next one (retries
//!   stay on the primary until it is marked dead for this fetch). A host the transport reports as not configured
//!   ([`Transport::host_configured`]: no live index behind it yet) is skipped without a request (audit P0-3).
//! - **Parallel ranges**: a blob is fetched as [`RANGE_BYTES`] ranges, [`PARALLEL_RANGES`] at a time, written in
//!   place into a preallocated `store/.tmp/<sha256>.part`; [`PARALLEL_BLOBS`] blobs go at once.
//! - **Dead sources cost one round** (N176, ADR 0159): once a range from a source fails, that blob starts no more
//!   ranges from it; once a module host has failed [`DEAD_HOST_AFTER`] blobs and served none, the rest of this fetch
//!   skips it. An unpublished or unreachable host (a firewall that drops packets, a captive portal) then delays the
//!   first install by about one timeout per host instead of one per range of every blob.
//! - **Resumable**: finished range numbers are appended to `<sha256>.done`, each line in one write after its bytes
//!   are synced; a restart (a quit, a crash, a kill) fetches only the rest. Crash recovery (backlog P2, ADR 0218):
//!   only whole lines count and a torn last line is dropped before anything is appended (a kill between a mark's
//!   digits and its newline used to merge it with the next mark: "1" + "0" read as range 10); a part a crash left
//!   read-only is made writable; and a part kept from an earlier fetch that fails its sha256 starts over once from
//!   the same source instead of failing a model's only (pinned) source.
//! - **Progress counts each byte once** (N177, ADR 0165): a blob's bytes already reported in this fetch (ranges a
//!   failed source finished) aren't added again when the next source resumes its part.
//! - **Verified**: a blob enters the store only after its whole size and sha256 match ([`Store::insert_file`]);
//!   a mismatch discards the part and its ranges.
//!
//! The fetcher has no network code: bytes come through [`Transport::get_range`] (qrtz-app's HTTPS transport over
//! qrtz-speech's downloader, or a `.qrtzpkg`).
use crate::index::BlobRef;
use crate::store::Store;
use futures_util::future::{BoxFuture, FutureExt};
use futures_util::stream::{self, StreamExt};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// In a fetch error, a source whose bytes didn't hash to the signed sha256 (callers report it as a checksum error).
pub const MISMATCH: &str = "bytes didn't match";
pub const RANGE_BYTES: u64 = 8 * 1024 * 1024;
pub const PARALLEL_RANGES: usize = 4;
pub const PARALLEL_BLOBS: usize = 3;
/// A module host that has failed this many blobs in one fetch, and served none, is skipped for the rest of it.
pub const DEAD_HOST_AFTER: u64 = 2;

/// Where one blob's bytes can come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Module host `n` (0 = primary), at `v1/blobs/sha256/<hex>`.
    Host(usize),
    /// An extra mirror URL from the signed tree.
    Url(String),
}

/// A source of layout paths and byte ranges: the HTTPS hosts or a `.qrtzpkg`.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    fn hosts(&self) -> usize;
    /// Whether module host `host` is set up to serve blobs. A host that isn't (the pinned hosts before the signed
    /// index exists) is never asked for a blob, so a pinned model goes straight to its own mirror URL.
    fn host_configured(&self, _host: usize) -> bool {
        true
    }
    /// A small document; `Ok(None)` when the host doesn't have it.
    async fn get(&self, host: usize, path: &str, max: usize) -> Result<Option<Vec<u8>>, String>;
    /// Exactly `len` bytes of blob `sha256` starting at `start`, from `src`. Anything else is an error.
    async fn get_range(&self, src: &Source, sha256: &str, start: u64, len: u64) -> Result<Vec<u8>, String>;
}

fn range_count(size: u64) -> u64 {
    size.div_ceil(RANGE_BYTES)
}

/// The range numbers in `<sha256>.done`, and whether the file is exactly those lines. Only lines that end in a
/// newline count (backlog P2, ADR 0218): a process killed between a mark's digits and its newline leaves a torn last
/// line whose range is simply fetched again. Bytes that aren't a number (a crash's garbage) are dropped the same way.
fn read_marks(p: &Path) -> (BTreeSet<u64>, bool) {
    let raw = std::fs::read(p).unwrap_or_default();
    let text = String::from_utf8_lossy(&raw);
    let mut clean = true;
    let mut set = BTreeSet::new();
    for l in text.split_inclusive('\n') {
        match l.strip_suffix('\n').and_then(|n| n.parse::<u64>().ok()) {
            Some(i) => {
                set.insert(i);
            }
            None => clean = false,
        }
    }
    (set, clean)
}

fn done_set(p: &Path) -> BTreeSet<u64> {
    read_marks(p).0
}

/// Append one finished range's mark in a single write, so a kill can't leave its digits without their newline.
fn append_mark(p: &Path, i: u64) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
    f.write_all(format!("{i}\n").as_bytes())
}

#[cfg(unix)]
fn write_at(p: &Path, off: u64, b: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    let f = std::fs::OpenOptions::new().write(true).open(p)?;
    f.write_all_at(b, off)?;
    f.sync_data()
}

#[cfg(not(unix))]
fn write_at(p: &Path, off: u64, b: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(p)?;
    f.seek(SeekFrom::Start(off))?;
    f.write_all(b)?;
    f.sync_data()
}

/// Fetch every blob missing from the store. `progress(done, total)` counts bytes, blobs already stored included.
pub async fn fetch_blobs(
    t: &dyn Transport,
    store: &Store,
    blobs: &[BlobRef],
    progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<(), String> {
    let total: u64 = blobs.iter().map(|b| b.size).sum();
    let done = AtomicU64::new(0);
    crate::fsutil::mkdir_private(&store.tmp()).map_err(|e| e.to_string())?;
    // Boxed (`Send`) futures built up front: a closure in `StreamExt::map` makes the caller's future non-`Send`
    // under the async-trait lifetimes (rust-lang/rust#102211).
    let done = &done;
    // Per module host: blobs it failed, blobs it served (N176).
    let failed: Vec<AtomicU64> = (0..t.hosts()).map(|_| AtomicU64::new(0)).collect();
    let served: Vec<AtomicU64> = (0..t.hosts()).map(|_| AtomicU64::new(0)).collect();
    let (failed, served) = (&failed, &served);
    let futs: Vec<BoxFuture<'_, Result<(), String>>> = blobs
        .iter()
        .map(|b| {
            async move {
                if store.has(&b.sha256, b.size)
                    && store.verify(&b.sha256, b.size).map_err(|e| e.to_string())?
                {
                    progress(done.fetch_add(b.size, Ordering::SeqCst) + b.size, total);
                    return Ok(());
                }
                let mut errs = vec![];
                // This blob's bytes already in `done` during this fetch, across its sources (N177).
                let counted = AtomicU64::new(0);
                // Backlog P2 (ADR 0218): ranges a part kept from an earlier fetch (a quit, a crash, a kill) may be
                // what's wrong if the finished part fails its hash; the part is gone then, so that source gets one
                // fresh try before the next source (a model file has only its pinned one).
                let mut from_disk = kept_ranges(store, b);
                let srcs = sources(t, b);
                let mut k = 0;
                while let Some(src) = srcs.get(k).cloned() {
                    k += 1;
                    let host = match src {
                        Source::Host(i) => Some(i),
                        Source::Url(_) => None,
                    };
                    if let Some(i) = host {
                        let f = failed[i].load(Ordering::SeqCst);
                        if f >= DEAD_HOST_AFTER && served[i].load(Ordering::SeqCst) == 0 {
                            errs.push(format!("{src:?}: skipped, failed {f} blobs in this fetch"));
                            continue;
                        }
                    }
                    let r = fetch_one(t, store, b, &src, done, &counted, total, progress).await;
                    if let Some(i) = host {
                        let n = if matches!(r, Ok(true)) { served } else { failed };
                        n[i].fetch_add(1, Ordering::SeqCst);
                    }
                    match r {
                        Ok(true) => return Ok(()),
                        Ok(false) if from_disk => {
                            from_disk = false;
                            k -= 1;
                            errs.push(format!(
                                "{src:?}: {MISMATCH} (resumed from an earlier download; started over)"
                            ));
                        }
                        Ok(false) => errs.push(format!("{src:?}: {MISMATCH}")),
                        Err(e) => errs.push(format!("{src:?}: {e}")),
                    }
                }
                Err(format!("{}: every source failed ({})", b.sha256, errs.join("; ")))
            }
            .boxed()
        })
        .collect();
    let results: Vec<Result<(), String>> = stream::iter(futs)
        .buffer_unordered(PARALLEL_BLOBS)
        .collect()
        .await;
    let errs: Vec<String> = results.into_iter().filter_map(Result::err).collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

/// Whether `b` has a full-size part with finished ranges from before this fetch.
fn kept_ranges(store: &Store, b: &BlobRef) -> bool {
    let part = store.tmp().join(format!("{}.part", b.sha256));
    let marks = store.tmp().join(format!("{}.done", b.sha256));
    std::fs::metadata(&part).is_ok_and(|m| m.len() == b.size) && !done_set(&marks).is_empty()
}

/// Where `b` is fetched from. ADR 0171: a model file's bytes are never on modules.qrtz.ai; the signed tree gives it
/// pinned Hugging Face URLs, and it comes only from those (in order). Every other blob (code, package trees,
/// manifests) comes only from the configured module hosts. Either way the bytes are checked against the signed sha256.
pub fn sources(t: &dyn Transport, b: &BlobRef) -> Vec<Source> {
    if !b.mirrors.is_empty() {
        return b.mirrors.iter().cloned().map(Source::Url).collect();
    }
    (0..t.hosts())
        .filter(|h| t.host_configured(*h))
        .map(Source::Host)
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn fetch_one(
    t: &dyn Transport,
    store: &Store,
    b: &BlobRef,
    src: &Source,
    done: &AtomicU64,
    counted: &AtomicU64,
    total: u64,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<bool, String> {
    let io = |e: std::io::Error| e.to_string();
    let part = store.tmp().join(format!("{}.part", b.sha256));
    let marks = store.tmp().join(format!("{}.done", b.sha256));
    if std::fs::metadata(&part).map(|m| m.len()).ok() != Some(b.size) {
        let _ = std::fs::remove_file(&marks);
        let mut o = std::fs::OpenOptions::new();
        o.create(true).write(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        o.open(&part).map_err(io)?.set_len(b.size).map_err(io)?;
    } else {
        // Kept from an earlier fetch. A crash between `insert_file`'s chmod and its rename leaves it 0444, and every
        // range write would then fail on each retry (backlog P2, ADR 0218).
        crate::fsutil::set_mode(&part, 0o600).map_err(io)?;
    }
    let (have, clean) = read_marks(&marks);
    if !clean {
        // A torn or garbled line: rewrite the marks as whole lines before anything is appended to them.
        let lines: String = have.iter().map(|i| format!("{i}\n")).collect();
        crate::fsutil::write_private_atomic(&marks, lines.as_bytes()).map_err(|e| e.to_string())?;
    }
    let n = range_count(b.size);
    let already: u64 = have
        .iter()
        .filter(|i| **i < n)
        .map(|i| (b.size - i * RANGE_BYTES).min(RANGE_BYTES))
        .sum();
    // Only what this fetch hasn't reported yet: after a source switch the finished ranges were counted as they
    // landed (N177); a part that had to start over takes its bytes back out.
    let before = counted.swap(already, Ordering::SeqCst);
    if already >= before {
        let d = already - before;
        progress(done.fetch_add(d, Ordering::SeqCst) + d, total);
    } else {
        let d = before - already;
        progress(done.fetch_sub(d, Ordering::SeqCst) - d, total);
    }
    let log = Mutex::new(());
    let pending: Vec<u64> = (0..n).filter(|i| !have.contains(i)).collect();
    // Once a range from this source fails, ranges after the first round don't ask it (N176). The first round
    // always runs, so a one-off failed range still leaves its neighbours on disk for the resume.
    let gave_up = AtomicBool::new(false);
    let gave_up = &gave_up;
    let (part_r, marks_r, log_r) = (&part, &marks, &log);
    let futs: Vec<BoxFuture<'_, Result<(), String>>> = pending
        .into_iter()
        .enumerate()
        .map(|(k, i)| {
            let (part, marks, log) = (part_r, marks_r, log_r);
            async move {
                let start = i * RANGE_BYTES;
                let len = (b.size - start).min(RANGE_BYTES);
                if k >= PARALLEL_RANGES && gave_up.load(Ordering::SeqCst) {
                    return Err("an earlier range from this source failed".into());
                }
                let bytes = match t.get_range(src, &b.sha256, start, len).await {
                    Ok(b) => b,
                    Err(e) => {
                        gave_up.store(true, Ordering::SeqCst);
                        return Err(e);
                    }
                };
                if bytes.len() as u64 != len {
                    gave_up.store(true, Ordering::SeqCst);
                    return Err(format!("range {i}: {} bytes, wanted {len}", bytes.len()));
                }
                write_at(part, start, &bytes).map_err(|e| e.to_string())?;
                {
                    let _g = log.lock().expect("log");
                    append_mark(marks, i).map_err(|e| e.to_string())?;
                }
                counted.fetch_add(len, Ordering::SeqCst);
                progress(done.fetch_add(len, Ordering::SeqCst) + len, total);
                Ok(())
            }
            .boxed()
        })
        .collect();
    let results: Vec<Result<(), String>> = stream::iter(futs)
        .buffer_unordered(PARALLEL_RANGES)
        .collect()
        .await;
    // The first real error, not a range that only stopped because of it.
    let mut errs: Vec<String> = results.into_iter().filter_map(Result::err).collect();
    errs.sort_by_key(|e| e.starts_with("an earlier range"));
    if let Some(e) = errs.into_iter().next() {
        return Err(e);
    }
    let ok = store.insert_file(&part, &b.sha256, b.size).map_err(io)?;
    let _ = std::fs::remove_file(&marks);
    if !ok {
        // The bytes counted for this blob were wrong: take them back out of the progress.
        done.fetch_sub(counted.swap(0, Ordering::SeqCst), Ordering::SeqCst);
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Two module hosts (only `configured` of them set up) and any mirror URL, all serving `data`; every request
    /// is recorded.
    struct Mock {
        data: Vec<u8>,
        configured: usize,
        asked: Mutex<Vec<Source>>,
    }

    #[async_trait::async_trait]
    impl Transport for Mock {
        fn hosts(&self) -> usize {
            2
        }
        fn host_configured(&self, host: usize) -> bool {
            host < self.configured
        }
        async fn get(&self, _host: usize, _path: &str, _max: usize) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        async fn get_range(
            &self,
            src: &Source,
            _sha256: &str,
            start: u64,
            len: u64,
        ) -> Result<Vec<u8>, String> {
            self.asked.lock().unwrap().push(src.clone());
            let (s, e) = (start as usize, (start + len) as usize);
            Ok(self.data[s..e].to_vec())
        }
    }

    fn blob(data: &[u8]) -> BlobRef {
        BlobRef {
            sha256: hex::encode(Sha256::digest(data)),
            size: data.len() as u64,
            mirrors: vec!["https://huggingface.co/r/resolve/abc/f.bin".into()],
        }
    }

    async fn run(configured: usize) -> Vec<Source> {
        let data = b"qrtz model bytes".to_vec();
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("store"));
        let t = Mock {
            data: data.clone(),
            configured,
            asked: Mutex::new(vec![]),
        };
        fetch_blobs(&t, &store, &[blob(&data)], &|_, _| {}).await.unwrap();
        assert!(store.has(&blob(&data).sha256, data.len() as u64));
        t.asked.into_inner().unwrap()
    }

    #[tokio::test]
    async fn hosts_that_arent_configured_get_no_request() {
        // P0-3: before modules.qrtz.ai and the B2 mirror exist, a pinned model goes straight to its mirror URL.
        let asked = run(0).await;
        assert!(!asked.is_empty());
        assert!(asked.iter().all(|s| matches!(s, Source::Url(_))), "{asked:?}");
    }

    #[tokio::test]
    async fn a_model_file_never_asks_the_module_host() {
        // ADR 0171: even with modules.qrtz.ai configured, model bytes come from their pinned Hugging Face URL.
        let asked = run(1).await;
        assert!(!asked.is_empty());
        assert!(asked.iter().all(|s| matches!(s, Source::Url(_))), "{asked:?}");
    }

    #[test]
    fn sources_skip_unconfigured_hosts_in_order() {
        let t = Mock {
            data: vec![],
            configured: 1,
            asked: Mutex::new(vec![]),
        };
        let b = blob(b"x");
        assert_eq!(sources(&t, &b), vec![Source::Url(b.mirrors[0].clone())]);
        let code = BlobRef { mirrors: vec![], ..b };
        assert_eq!(sources(&t, &code), vec![Source::Host(0)]);
    }
}
