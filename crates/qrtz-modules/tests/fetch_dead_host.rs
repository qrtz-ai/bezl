//! N176 (ADR 0159): a module host that never answers costs the first install about one round per host, not one
//! request per range of every blob. Echo's first-run Parakeet download (21 files, 75 ranges) goes through this
//! fetcher with `modules.qrtz.ai` and its mirror ahead of the pinned Hugging Face URL.
//!
//! ADR 0171: a model file (a blob with Hugging Face URLs) no longer asks any module host at all, so with these blobs
//! the dead hosts get no request; the rule still holds for the blobs hosts serve (code, trees, manifests).
use qrtz_modules::fetch::{
    fetch_blobs, Source, Transport, DEAD_HOST_AFTER, PARALLEL_BLOBS, PARALLEL_RANGES, RANGE_BYTES,
};
use qrtz_modules::index::BlobRef;
use qrtz_modules::store::Store;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

const MIRROR: &str = "https://huggingface.co/m";

/// Hosts 0 and 1 fail every range after a short wait (an unreachable host, scaled down from a 15 s connect
/// timeout); the blob's mirror URL serves.
struct DeadHosts {
    blobs: BTreeMap<String, Vec<u8>>,
    host_calls: [AtomicU64; 2],
    mirror_calls: AtomicU64,
}

#[async_trait::async_trait]
impl Transport for DeadHosts {
    fn hosts(&self) -> usize {
        2
    }
    async fn get(&self, _host: usize, _path: &str, _max: usize) -> Result<Option<Vec<u8>>, String> {
        Err("unreachable".into())
    }
    async fn get_range(&self, src: &Source, sha256: &str, start: u64, len: u64) -> Result<Vec<u8>, String> {
        match src {
            Source::Host(i) => {
                self.host_calls[*i].fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                Err("connect timed out".into())
            }
            Source::Url(u) => {
                assert_eq!(u, &format!("{MIRROR}/{sha256}"));
                self.mirror_calls.fetch_add(1, Ordering::SeqCst);
                let b = &self.blobs[sha256];
                Ok(b[start as usize..(start + len) as usize].to_vec())
            }
        }
    }
}

fn sha(b: &[u8]) -> String {
    qrtz_modules::sha256_hex(b)
}

fn setup(sizes: &[usize]) -> (DeadHosts, Vec<BlobRef>) {
    let mut blobs = BTreeMap::new();
    let mut refs = vec![];
    for (n, size) in sizes.iter().enumerate() {
        let b: Vec<u8> = (0..*size).map(|i| (i as u64 * 31 + n as u64 * 7) as u8).collect();
        let h = sha(&b);
        refs.push(BlobRef {
            sha256: h.clone(),
            size: b.len() as u64,
            mirrors: vec![format!("{MIRROR}/{h}")],
        });
        blobs.insert(h, b);
    }
    let t = DeadHosts {
        blobs,
        host_calls: [AtomicU64::new(0), AtomicU64::new(0)],
        mirror_calls: AtomicU64::new(0),
    };
    (t, refs)
}

#[tokio::test]
async fn a_blob_stops_asking_a_source_after_its_first_failed_range() {
    // One big file (the shape of Parakeet's 445 MB encoder, 56 ranges), scaled to 9 ranges.
    let (t, refs) = setup(&[(8 * RANGE_BYTES + 1) as usize]);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    fetch_blobs(&t, &store, &refs, &|_, _| {}).await.unwrap();
    assert!(store.verify(&refs[0].sha256, refs[0].size).unwrap());
    for h in &t.host_calls {
        let n = h.load(Ordering::SeqCst);
        // Without the fix every one of the 9 ranges asks each dead host.
        assert!(
            n <= PARALLEL_RANGES as u64,
            "a dead host was asked {n} times for one blob"
        );
    }
    assert_eq!(t.mirror_calls.load(Ordering::SeqCst), 9);
}

#[tokio::test]
async fn a_host_that_failed_every_blob_so_far_is_skipped_for_the_rest_of_the_fetch() {
    // Echo's set is 21 files; 12 small ones keep the test fast.
    let (t, refs) = setup(&[1000; 12]);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    let t0 = std::time::Instant::now();
    fetch_blobs(&t, &store, &refs, &|_, _| {}).await.unwrap();
    let took = t0.elapsed();
    for r in &refs {
        assert!(store.verify(&r.sha256, r.size).unwrap());
    }
    // The first blobs run together, so up to PARALLEL_BLOBS of them (at least DEAD_HOST_AFTER) find each host
    // dead before the rest skip it. Without the fix: 12 per host.
    let bound = (PARALLEL_BLOBS as u64).max(DEAD_HOST_AFTER) + 1;
    for h in &t.host_calls {
        let n = h.load(Ordering::SeqCst);
        assert!(n <= bound, "a dead host was asked {n} times for 12 blobs");
    }
    assert_eq!(t.mirror_calls.load(Ordering::SeqCst), 12);
    // 12 blobs, 3 at a time, 2 dead hosts at 20 ms: 4 rounds x 40 ms without the skip; one round with it.
    assert!(took < std::time::Duration::from_millis(150), "took {took:?}");
}

#[tokio::test]
async fn a_model_blob_never_asks_a_module_host() {
    // ADR 0171: even a host that has the bytes is never asked for a model file; its Hugging Face URL serves.
    struct Partial {
        inner: DeadHosts,
        missing: Vec<String>,
        served: AtomicU64,
    }
    #[async_trait::async_trait]
    impl Transport for Partial {
        fn hosts(&self) -> usize {
            1
        }
        async fn get(&self, _h: usize, _p: &str, _m: usize) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        async fn get_range(
            &self,
            src: &Source,
            sha256: &str,
            start: u64,
            len: u64,
        ) -> Result<Vec<u8>, String> {
            match src {
                Source::Host(0) if !self.missing.iter().any(|m| m == sha256) => {
                    self.served.fetch_add(1, Ordering::SeqCst);
                    let b = &self.inner.blobs[sha256];
                    Ok(b[start as usize..(start + len) as usize].to_vec())
                }
                Source::Host(_) => Err("http 404 for a range".into()),
                u => self.inner.get_range(u, sha256, start, len).await,
            }
        }
    }
    let (inner, refs) = setup(&[500; 8]);
    let t = Partial {
        missing: vec![refs[0].sha256.clone(), refs[1].sha256.clone()],
        inner,
        served: AtomicU64::new(0),
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    // One blob at a time would make the order deterministic; with 3 at once the two misses can come first, so
    // the host may be skipped after them. Fetch the misses last to pin the rule: served once, never skipped.
    let mut order = refs.clone();
    order.rotate_left(2);
    fetch_blobs(&t, &store, &order, &|_, _| {}).await.unwrap();
    assert_eq!(t.served.load(Ordering::SeqCst), 0);
    assert_eq!(t.inner.mirror_calls.load(Ordering::SeqCst), 8);
}
