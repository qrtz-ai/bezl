//! Cycle 21: the search-model download is bounded (N99) and a panicking embedder load does not wedge reloads
//! (N102). Every server here is a local scripted socket; nothing touches the network.
#![cfg_attr(not(feature = "minilm-download"), allow(unused_imports))]
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use qrtz_model::LifecycleError;
use qrtz_search::embedder::{Embedder, LazyEmbedder, LoadFuture};

struct Fixed;

#[async_trait]
impl Embedder for Fixed {
    fn id(&self) -> &str {
        "fixed"
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
}

/// N102: a loader that panics once. Without the fix the load loop dies with `running` set and the second reload
/// never resolves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n102_a_panicking_load_fails_that_load_and_the_next_reload_still_runs() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let lazy = LazyEmbedder::new(Box::new(move || -> LoadFuture {
        let n = c.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if n == 0 {
                panic!("candle blew up");
            }
            Ok(Arc::new(Fixed) as Arc<dyn Embedder>)
        })
    }));
    let first = tokio::time::timeout(Duration::from_secs(5), lazy.reload()).await;
    assert_eq!(first, Ok(false), "the panicking load reports failure");
    assert!(lazy.current().is_none());
    let second = tokio::time::timeout(Duration::from_secs(5), lazy.reload()).await;
    assert_eq!(second, Ok(true), "a later reload still loads");
    assert_eq!(
        lazy.current().map(|e| e.id().to_string()).as_deref(),
        Some("fixed")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[cfg(feature = "minilm-download")]
mod download {
    use super::*;
    use qrtz_search::minilm::{download_files, DownloadLimits};
    use sha2::{Digest, Sha256};

    #[derive(Clone)]
    enum Reply {
        /// 200 with this body and a content-length.
        Body(Vec<u8>),
        /// 200 with no content-length: `n` bytes, then close.
        Unsized(usize),
        /// 200 claiming `len`, sends `sent` bytes, then holds the connection open.
        StallBody { len: usize, sent: usize },
        /// Reads the request and never answers.
        StallHead,
        /// 200 with a content-length of `n` and no body sent (refused from the header).
        Claims(usize),
    }

    /// Serves `/<repo>/resolve/<rev>/<name>` by name; returns the base URL.
    fn serve(map: Vec<(&'static str, Reply)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in listener.incoming() {
                let Ok(mut s) = s else { return };
                let map = map.clone();
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut r = BufReader::new(s.try_clone().unwrap());
                    if r.read_line(&mut line).is_err() {
                        return;
                    }
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
                            break;
                        }
                    }
                    let name = line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .rsplit('/')
                        .next()
                        .unwrap_or("");
                    let reply = map.iter().find(|(n, _)| *n == name).map(|(_, r)| r.clone());
                    let ok = "HTTP/1.1 200 OK\r\nconnection: close\r\n";
                    let _ = match reply {
                        Some(Reply::Body(b)) => s
                            .write_all(format!("{ok}content-length: {}\r\n\r\n", b.len()).as_bytes())
                            .and_then(|_| s.write_all(&b)),
                        Some(Reply::Unsized(n)) => s
                            .write_all(format!("{ok}\r\n").as_bytes())
                            .and_then(|_| s.write_all(&vec![b'x'; n])),
                        Some(Reply::StallBody { len, sent }) => {
                            let r = s
                                .write_all(format!("{ok}content-length: {len}\r\n\r\n").as_bytes())
                                .and_then(|_| s.write_all(&vec![b'x'; sent]));
                            std::thread::sleep(Duration::from_secs(10));
                            r
                        }
                        Some(Reply::StallHead) => {
                            std::thread::sleep(Duration::from_secs(10));
                            Ok(())
                        }
                        Some(Reply::Claims(n)) => {
                            s.write_all(format!("{ok}content-length: {n}\r\n\r\n").as_bytes())
                        }
                        None => s.write_all(
                            b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        ),
                    };
                });
            }
        });
        base
    }

    fn sha(b: &[u8]) -> String {
        format!("{:x}", Sha256::digest(b))
    }

    fn limits() -> DownloadLimits {
        DownloadLimits {
            max_file_bytes: 1000,
            idle: Duration::from_millis(300),
        }
    }

    fn names(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn n99_good_files_land_under_their_names_with_no_part_left() {
        let (a, b) = (b"{\"a\":1}".to_vec(), vec![7u8; 900]);
        let base = serve(vec![
            ("a.json", Reply::Body(a.clone())),
            ("b.bin", Reply::Body(b.clone())),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let (ha, hb) = (sha(&a), sha(&b));
        let files = [("a.json", ha.as_str()), ("b.bin", hb.as_str())];
        download_files(&reqwest::Client::new(), &base, dir.path(), &files, limits())
            .await
            .unwrap();
        assert_eq!(names(dir.path()), ["a.json", "b.bin"]);
        assert_eq!(std::fs::read(dir.path().join("b.bin")).unwrap(), b);
    }

    #[tokio::test]
    async fn n99_a_body_that_stalls_fails_within_the_idle_bound_and_leaves_nothing() {
        let base = serve(vec![("m.bin", Reply::StallBody { len: 500, sent: 100 })]);
        let dir = tempfile::tempdir().unwrap();
        let files = [("m.bin", "00")];
        let t = Instant::now();
        let e = download_files(&reqwest::Client::new(), &base, dir.path(), &files, limits())
            .await
            .unwrap_err();
        assert!(e.starts_with("m.bin: body: no data"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
    }

    #[tokio::test]
    async fn n99_a_server_that_never_answers_fails_within_the_idle_bound() {
        let base = serve(vec![("m.bin", Reply::StallHead)]);
        let dir = tempfile::tempdir().unwrap();
        let t = Instant::now();
        let e = download_files(
            &reqwest::Client::new(),
            &base,
            dir.path(),
            &[("m.bin", "00")],
            limits(),
        )
        .await
        .unwrap_err();
        assert!(e.starts_with("m.bin: request: no data"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn n99_an_oversized_file_is_refused_by_header_or_by_count() {
        let base = serve(vec![
            ("big.bin", Reply::Claims(5000)),
            ("chunked.bin", Reply::Unsized(3000)),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let c = reqwest::Client::new();
        let e = download_files(&c, &base, dir.path(), &[("big.bin", "00")], limits())
            .await
            .unwrap_err();
        assert_eq!(e, "big.bin: larger than 1000 bytes");
        let e = download_files(&c, &base, dir.path(), &[("chunked.bin", "00")], limits())
            .await
            .unwrap_err();
        assert_eq!(e, "chunked.bin: larger than 1000 bytes");
        assert!(names(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn n99_a_bad_second_file_keeps_the_good_first_and_a_retry_fetches_only_the_rest() {
        let a = b"first".to_vec();
        let base = serve(vec![
            ("a.txt", Reply::Body(a.clone())),
            ("b.txt", Reply::Body(b"tampered".to_vec())),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let (ha, hb) = (sha(&a), sha(b"genuine"));
        let files = [("a.txt", ha.as_str()), ("b.txt", hb.as_str())];
        let e = download_files(&reqwest::Client::new(), &base, dir.path(), &files, limits())
            .await
            .unwrap_err();
        assert!(e.starts_with("b.txt: sha256 "), "{e}");
        assert_eq!(names(dir.path()), ["a.txt"]);
    }
}
