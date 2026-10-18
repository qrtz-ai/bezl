//! `qrtz-media://rec/<name>.wav`: recordings for the renderer's <audio>, nothing else. The name is the file name
//! of a meeting's `audio_path` (the renderer's `audioName`): a request is served only when a meeting row owns a
//! recording by that name (N140), so a restored `<id>-restored-<ms>.wav` plays and an orphan `<uuid>.wav` no row
//! references is 404. Supports single byte ranges so WebKit can seek.
//!
//! A Tauri protocol response is one in-memory body, so no response is larger than [`MAX_CHUNK`]: a request
//! with no Range header for a bigger file gets `206` with the first chunk, and a range (WebKit asks for
//! `bytes=0-`) is cut to a chunk from its start. The player reads `Content-Range` and asks for the rest as it
//! plays or seeks, so memory stays bounded however long the recording is (a 2 h WAV is about 230 MB).
//!
//! A meeting whose delete is pending (undo window, or a deferred delete that failed and waits for the next launch)
//! is 404, like the lists that hide it (N133). Requests run on a small fixed pool ([`spawn`], N135), not a thread
//! each.
use qrtz_storage::PendingDeletes;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use tauri::http::{header, Request, Response, StatusCode};

/// A plain recording file name: `[A-Za-z0-9][A-Za-z0-9._-]*\.wav`, at most 128 bytes. No separator, no `..`
/// prefix, no percent escapes: the file is always looked up inside the recordings directory by this name.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (5..=128).contains(&b.len())
        && name.ends_with(".wav")
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// The meeting that owns a recording file name (its `audio_path` ends in that name), if any (N140). The app
/// asks storage (`NoteRepo::meeting_id_by_audio_name`).
pub type Owner<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A `Range` header against a file of `len` bytes (RFC 9110 §14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// Serve as if there were no header: an unknown unit, several ranges, or an invalid one (inverted, not
    /// numbers). RFC 9110 has a server ignore those.
    Ignore,
    /// A valid single range nothing of the file satisfies: 416.
    Unsatisfiable,
    /// Inclusive byte range inside the file.
    Bytes(u64, u64),
}

/// Parses `bytes=a-b` / `bytes=a-` / `bytes=-n` (the unit case-insensitive) against `len`.
pub fn parse_range(h: &str, len: u64) -> Range {
    use Range::*;
    let Some((unit, spec)) = h.split_once('=') else {
        return Ignore;
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") || spec.contains(',') {
        return Ignore;
    }
    let Some((a, b)) = spec.split_once('-') else {
        return Ignore;
    };
    // Digits only (`u64::from_str` would take `+1`); too large for u64 is still a valid, huge number.
    let num = |s: &str| {
        (!s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())).then(|| s.parse().unwrap_or(u64::MAX))
    };
    match (a.trim(), b.trim()) {
        ("", "") => Ignore,
        ("", n) => match num(n) {
            None => Ignore,
            Some(0) => Unsatisfiable,
            Some(_) if len == 0 => Unsatisfiable,
            Some(n) => Bytes(len.saturating_sub(n), len - 1),
        },
        (s, e) => {
            let Some(s) = num(s) else { return Ignore };
            let e = match e {
                "" => u64::MAX,
                e => match num(e) {
                    Some(e) if e >= s => e,
                    _ => return Ignore,
                },
            };
            if s >= len {
                return Unsatisfiable;
            }
            Bytes(s, e.min(len - 1))
        }
    }
}

/// The most bytes one response carries.
pub const MAX_CHUNK: u64 = 4 * 1024 * 1024;

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).unwrap()
}

/// Request threads: WebKit fires several ranges per seek; more just wait their turn.
pub const WORKERS: usize = 4;

type Job = Box<dyn FnOnce() + Send>;

/// Runs `job` on the media pool ([`WORKERS`] threads, started on first use).
pub fn spawn(job: impl FnOnce() + Send + 'static) {
    static POOL: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    let tx = POOL.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..WORKERS {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("qrtz-media-{i}"))
                .spawn(move || loop {
                    let job = match rx.lock().unwrap_or_else(|p| p.into_inner()).recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    };
                    // A panicking request must not take a worker with it.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                })
                .expect("media worker thread");
        }
        tx
    });
    let _ = tx.send(Box::new(job));
}

/// `owner`: which meeting has this recording (none: 404, N140). `pending`: the app's tombstones (`PendingDeletes`
/// on the data dir); an owner on it is 404 (N133).
pub fn serve(
    recordings: &Path,
    pending: &PendingDeletes,
    owner: Owner<'_>,
    req: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    // macOS/Linux: qrtz-media://rec/<name>; Windows: http://qrtz-media.localhost/rec/<name>.
    let path = req.uri().path().trim_start_matches('/');
    let name = path.strip_prefix("rec/").unwrap_or(path);
    let name = name.replace("%2D", "-");
    if !valid_name(&name) {
        return status(StatusCode::FORBIDDEN);
    }
    let Some(id) = owner(&name) else {
        return status(StatusCode::NOT_FOUND); // an orphan file, or no such recording
    };
    if pending.has(&id) {
        return status(StatusCode::NOT_FOUND);
    }
    // N132 (ADR 0086): the regular file under that name, never a symlink's target, a FIFO (would block this
    // thread for good) or a directory.
    let Ok((mut f, len)) = qrtz_storage::recordings::open_regular(&recordings.join(&name)) else {
        return status(StatusCode::NOT_FOUND);
    };
    serve_file(&mut f, len, req, MAX_CHUNK)
}

/// [`serve`] once the file is open; `chunk` is the response size cap ([`MAX_CHUNK`]; tests pass a small one).
fn serve_file(f: &mut std::fs::File, len: u64, req: &Request<Vec<u8>>, chunk: u64) -> Response<Vec<u8>> {
    let range = match req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|h| parse_range(h, len))
    {
        Some(Range::Unsatisfiable) => Some(None),
        Some(Range::Bytes(s, e)) => Some(Some((s, e))),
        // No Range, or one to ignore (N135), on a file over the cap: the first chunk as a partial response.
        None | Some(Range::Ignore) if len > chunk => Some(Some((0, len - 1))),
        None | Some(Range::Ignore) => None,
    };
    let b = Response::builder()
        .header(header::CONTENT_TYPE, "audio/wav")
        .header(header::ACCEPT_RANGES, "bytes");
    match range {
        Some(None) => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{len}"))
            .body(Vec::new())
            .unwrap(),
        Some(Some((s, e))) => {
            let e = e.min(s + chunk - 1);
            let mut buf = vec![0; (e - s + 1) as usize];
            if f.seek(SeekFrom::Start(s))
                .and_then(|_| f.read_exact(&mut buf))
                .is_err()
            {
                return status(StatusCode::INTERNAL_SERVER_ERROR);
            }
            b.status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {s}-{e}/{len}"))
                .header(header::CONTENT_LENGTH, buf.len())
                .body(buf)
                .unwrap()
        }
        None => {
            let mut buf = Vec::with_capacity(len as usize);
            if f.read_to_end(&mut buf).is_err() {
                return status(StatusCode::INTERNAL_SERVER_ERROR);
            }
            b.status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, buf.len())
                .body(buf)
                .unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule before N140 as a stand-in for the meeting rows: a lowercase `<uuid>.wav` belongs to `<uuid>`.
    fn uuid_owner(name: &str) -> Option<String> {
        let ok = name.len() == 40
            && name.ends_with(".wav")
            && name[..36]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-');
        ok.then(|| name[..36].to_string())
    }

    /// `serve` with no tombstones and [`uuid_owner`] (tests that aren't about rows or pending deletes).
    fn serve(recordings: &Path, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        super::serve(recordings, &PendingDeletes::new(recordings), &uuid_owner, req)
    }

    const ID: &str = "0b9f6a2e-6c1d-4e7a-9a43-2f1d0c8e5b71.wav";

    #[test]
    fn only_plain_wav_names() {
        assert!(valid_name(ID));
        assert!(valid_name(
            "0b9f6a2e-6c1d-4e7a-9a43-2f1d0c8e5b71-restored-1760745600000.wav"
        ));
        assert!(valid_name("2026-10-17-weekly-sync.wav"));
        for bad in [
            "../config.json",
            "..wav",
            ".hidden.wav",
            "a/b.wav",
            "a\\b.wav",
            "a%2Fb.wav",
            "a b.wav",
            ".wav",
            "x.wav.stitch",
            &ID.replace(".wav", ".mp3"),
            &format!("{}.wav", "a".repeat(125)),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn ranges() {
        use Range::*;
        assert_eq!(parse_range("bytes=0-", 10), Bytes(0, 9));
        assert_eq!(parse_range("bytes=2-4", 10), Bytes(2, 4));
        assert_eq!(parse_range("bytes=-3", 10), Bytes(7, 9));
        assert_eq!(parse_range("bytes=5-99", 10), Bytes(5, 9));
        assert_eq!(parse_range("Bytes=2-4", 10), Bytes(2, 4));
        assert_eq!(parse_range("bytes=10-", 10), Unsatisfiable);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), Ignore);
    }

    #[test]
    fn serves_files_and_ranges_and_refuses_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ID), b"RIFF0123456789").unwrap();
        let get = |uri: &str, range: Option<&str>| {
            let mut r = Request::builder().uri(uri);
            if let Some(h) = range {
                r = r.header(header::RANGE, h);
            }
            serve(dir.path(), &r.body(Vec::new()).unwrap())
        };
        let full = get(&format!("qrtz-media://rec/{ID}"), None);
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.body(), b"RIFF0123456789");
        let part = get(
            &format!("http://qrtz-media.localhost/rec/{ID}"),
            Some("bytes=4-7"),
        );
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.body(), b"0123");
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 4-7/14");
        assert_eq!(
            get("qrtz-media://rec/..%2Fconfig.json", None).status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get("qrtz-media://rec/1b9f6a2e-6c1d-4e7a-9a43-2f1d0c8e5b71.wav", None).status(),
            StatusCode::NOT_FOUND
        );
    }

    fn req(range: Option<&str>) -> Request<Vec<u8>> {
        let mut r = Request::builder().uri(format!("qrtz-media://rec/{ID}"));
        if let Some(h) = range {
            r = r.header(header::RANGE, h);
        }
        r.body(Vec::new()).unwrap()
    }

    #[test]
    fn no_response_is_larger_than_one_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(ID);
        std::fs::write(&p, b"RIFF0123456789").unwrap();
        let get = |range: Option<&str>, chunk: u64| {
            let mut f = std::fs::File::open(&p).unwrap();
            serve_file(&mut f, 14, &req(range), chunk)
        };
        // No Range on a file over the cap: 206 with the first chunk, so the player asks for the rest.
        let first = get(None, 4);
        assert_eq!(first.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(first.body(), b"RIFF");
        assert_eq!(first.headers()[header::CONTENT_RANGE], "bytes 0-3/14");
        assert_eq!(first.headers()[header::CONTENT_LENGTH], "4");
        // Open-ended and large ranges are cut to a chunk from their start; small ones are served as asked.
        for (h, body, cr) in [
            ("bytes=0-", &b"RIFF"[..], "bytes 0-3/14"),
            ("bytes=6-", b"2345", "bytes 6-9/14"),
            ("bytes=2-13", b"FF01", "bytes 2-5/14"),
            ("bytes=-3", b"789", "bytes 11-13/14"),
            ("bytes=12-13", b"89", "bytes 12-13/14"),
        ] {
            let r = get(Some(h), 4);
            assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT, "{h}");
            assert_eq!(r.body(), body, "{h}");
            assert_eq!(r.headers()[header::CONTENT_RANGE], cr, "{h}");
        }
        // A file within the cap is still one plain 200.
        let whole = get(None, 14);
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(whole.body(), b"RIFF0123456789");
    }

    #[test]
    fn a_two_hour_recording_is_served_in_bounded_chunks() {
        // 2 h of 16 kHz 16-bit stereo is about 230 MB; a sparse file of that length costs no disk.
        let dir = tempfile::tempdir().unwrap();
        let len: u64 = 2 * 3600 * 16_000 * 2 * 2;
        let f = std::fs::File::create(dir.path().join(ID)).unwrap();
        f.set_len(len).unwrap();
        let mid = format!("bytes={}-", len / 2);
        for (range, start) in [(None, 0), (Some("bytes=0-"), 0), (Some(mid.as_str()), len / 2)] {
            let r = serve(dir.path(), &req(range));
            assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT, "{range:?}");
            assert_eq!(r.body().len() as u64, MAX_CHUNK, "{range:?}");
            assert_eq!(
                r.headers()[header::CONTENT_RANGE],
                format!("bytes {start}-{}/{len}", start + MAX_CHUNK - 1).as_str(),
            );
        }
        // The last bytes: a short tail chunk.
        let tail = serve(dir.path(), &req(Some(&format!("bytes={}-", len - 10))));
        assert_eq!(tail.body().len(), 10);
    }

    /// Hardening cycle 27 (N132, ADR 0086): what sits in the recordings dir under a recording's name is served only
    /// when it is a regular file; traversal spellings and malformed ranges never panic or leave the directory.
    #[cfg(unix)]
    #[test]
    fn only_a_regular_file_is_served_never_a_symlink_fifo_or_directory() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"not audio").unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join(ID)).unwrap();
        let r = serve(dir.path(), &req(None));
        assert_eq!(
            r.status(),
            StatusCode::NOT_FOUND,
            "followed a symlink out of the recordings dir"
        );
        assert!(r.body().is_empty());
        std::fs::remove_file(dir.path().join(ID)).unwrap();
        std::fs::create_dir(dir.path().join(ID)).unwrap();
        assert_eq!(serve(dir.path(), &req(None)).status(), StatusCode::NOT_FOUND);
        std::fs::remove_dir(dir.path().join(ID)).unwrap();
        // A FIFO: File::open would block until a writer came (the request thread, for good).
        let fifo = dir.path().join(ID);
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        let (tx, rx) = std::sync::mpsc::channel();
        let d = dir.path().to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(serve(&d, &req(None)).status());
        });
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("blocked on a FIFO"),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn traversal_spellings_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("rec");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(dir.path().join(ID), b"RIFF outside").unwrap();
        for uri in [
            format!("qrtz-media://rec/../{ID}"),
            format!("qrtz-media://rec/..%2F{ID}"),
            format!("qrtz-media://rec/x/../{ID}"),
            format!("qrtz-media://rec/%2e%2e/{ID}"),
            format!("qrtz-media://rec/.%2E/{ID}"),
            format!("http://qrtz-media.localhost/rec/rec/../{ID}"),
            format!(
                "qrtz-media://rec/{}",
                ID.replace('-', "%2D").replace(".wav", "%2Ewav")
            ),
            format!("qrtz-media://rec/{}", ID.to_uppercase()),
            format!("qrtz-media://rec/{ID}%00"),
            "qrtz-media://rec/".to_string(),
        ] {
            let r = serve(
                &inner,
                &Request::builder().uri(uri.as_str()).body(Vec::new()).unwrap(),
            );
            assert!(
                matches!(r.status(), StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
                "{uri}: {}",
                r.status()
            );
            assert!(r.body().is_empty(), "{uri}");
        }
    }

    #[test]
    fn malformed_and_edge_ranges_never_panic_and_never_overrun() {
        let len = 14u64;
        for h in [
            "bytes=",
            "bytes=-",
            "bytes=--1",
            "bytes=a-b",
            "bytes=5-3",
            "bytes=-0",
            "bytes=14-",
            "bytes=18446744073709551615-",
            "bytes=0-18446744073709551615",
            "bytes=-18446744073709551616",
            "bytes=0-1,2-3",
            "items=0-1",
            "Bytes=0-1",
            "bytes= 0 - 1 ",
            "bytes=+1-2",
            "bytes=\u{0}",
        ] {
            if let Range::Bytes(s, e) = parse_range(h, len) {
                assert!(s <= e && e < len, "{h}: {s}-{e}");
            }
        }
        assert_eq!(
            parse_range("bytes=-0", len),
            Range::Unsatisfiable,
            "a zero-length suffix is unsatisfiable"
        );
        assert_eq!(parse_range("bytes=0-0", len), Range::Bytes(0, 0));
        assert_eq!(parse_range("bytes=-99", len), Range::Bytes(0, 13));
        assert_eq!(parse_range("bytes=0-", 0), Range::Unsatisfiable);
        // An empty file: a plain 200 with no body, and any range is 416 with `*/0`.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ID), b"").unwrap();
        let r = serve(dir.path(), &req(None));
        assert_eq!(r.status(), StatusCode::OK);
        assert!(r.body().is_empty());
        let r = serve(dir.path(), &req(Some("bytes=0-")));
        assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(r.headers()[header::CONTENT_RANGE], "bytes */0");
    }

    /// Seeking through a long (sparse) recording: every chunk stays inside the file and under the cap, the last one
    /// ends at the last byte, and a range past the end is 416, never a short read turned into a 500.
    #[test]
    fn every_chunk_of_a_long_recording_stays_inside_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let len: u64 = 3 * 3600 * 16_000 * 2 * 2 + 44 + 3; // 3 h stereo + header + an odd tail
        std::fs::File::create(dir.path().join(ID))
            .unwrap()
            .set_len(len)
            .unwrap();
        let mut at = 0u64;
        let mut n = 0;
        while at < len {
            let r = serve(dir.path(), &req(Some(&format!("bytes={at}-"))));
            assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT, "at {at}");
            let got = r.body().len() as u64;
            assert!(got > 0 && got <= MAX_CHUNK);
            assert_eq!(
                r.headers()[header::CONTENT_RANGE],
                format!("bytes {at}-{}/{len}", at + got - 1).as_str()
            );
            at += got * 11; // a seek every 11 chunks keeps the test fast
            n += 1;
        }
        assert!(n > 10);
        let last = serve(dir.path(), &req(Some(&format!("bytes={}-", len - 1))));
        assert_eq!(last.body().len(), 1);
        let past = serve(dir.path(), &req(Some(&format!("bytes={len}-"))));
        assert_eq!(past.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    /// N135 (RFC 9110 §14.2): a Range with an unknown unit, several ranges or an invalid spec is ignored (served as
    /// if absent, still capped); only a valid range nothing satisfies is 416.
    #[test]
    fn ranges_to_ignore_are_served_as_if_absent_and_only_unsatisfiable_ones_are_416() {
        use Range::*;
        let len = 14u64;
        for h in [
            "items=0-1",
            "bytes=0-1,2-3",
            "bytes=5-3",
            "bytes=",
            "bytes=-",
            "bytes=--1",
            "bytes=a-b",
            "bytes=+1-2",
            "bytes=1-+2",
            "bytes",
            "bytes=\u{0}",
            "0-1",
        ] {
            assert_eq!(parse_range(h, len), Ignore, "{h}");
        }
        for h in [
            "bytes=14-",
            "bytes=99-100",
            "bytes=-0",
            "bytes=18446744073709551615-",
        ] {
            assert_eq!(parse_range(h, len), Unsatisfiable, "{h}");
        }
        assert_eq!(parse_range("bytes=-18446744073709551616", len), Bytes(0, 13));
        assert_eq!(parse_range("bytes= 0 - 1 ", len), Bytes(0, 1));

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(ID);
        std::fs::write(&p, b"RIFF0123456789").unwrap();
        let get = |range: &str, chunk: u64| {
            let mut f = std::fs::File::open(&p).unwrap();
            serve_file(&mut f, len, &req(Some(range)), chunk)
        };
        for h in ["items=0-1", "bytes=0-1,2-3", "bytes=5-3"] {
            let whole = get(h, MAX_CHUNK);
            assert_eq!(whole.status(), StatusCode::OK, "{h}");
            assert_eq!(whole.body(), b"RIFF0123456789", "{h}");
            // Still capped: over the cap it is the first chunk, like no Range at all.
            let first = get(h, 4);
            assert_eq!(first.status(), StatusCode::PARTIAL_CONTENT, "{h}");
            assert_eq!(first.headers()[header::CONTENT_RANGE], "bytes 0-3/14", "{h}");
            assert_eq!(first.body(), b"RIFF", "{h}");
        }
        let r = get("bytes=14-", MAX_CHUNK);
        assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(r.headers()[header::CONTENT_RANGE], "bytes */14");
    }

    /// N133: a meeting whose delete is pending (tombstoned) is not served; after an undo it is again.
    #[test]
    fn a_meeting_pending_delete_is_404_until_the_delete_is_undone() {
        let data = tempfile::tempdir().unwrap();
        let rec = data.path().join("recordings");
        std::fs::create_dir(&rec).unwrap();
        std::fs::write(rec.join(ID), b"RIFF0123456789").unwrap();
        let other = "1b9f6a2e-6c1d-4e7a-9a43-2f1d0c8e5b71.wav";
        std::fs::write(rec.join(other), b"RIFFother").unwrap();
        let pending = PendingDeletes::new(data.path());
        let get = |name: &str, range: Option<&str>| {
            let mut r = Request::builder().uri(format!("qrtz-media://rec/{name}"));
            if let Some(h) = range {
                r = r.header(header::RANGE, h);
            }
            super::serve(&rec, &pending, &uuid_owner, &r.body(Vec::new()).unwrap())
        };
        assert_eq!(get(ID, None).status(), StatusCode::OK);
        pending.add(&ID[..36]).unwrap();
        for range in [None, Some("bytes=0-"), Some("bytes=4-7")] {
            let r = get(ID, range);
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{range:?}");
            assert!(r.body().is_empty());
        }
        // Other meetings play on.
        assert_eq!(get(other, None).body(), b"RIFFother");
        assert!(pending.remove(&ID[..36]).unwrap());
        let r = get(ID, None);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.body(), b"RIFF0123456789");
    }

    /// N135: requests share a fixed pool; a burst uses at most [`WORKERS`] threads, every job runs, and a
    /// panicking one doesn't take a worker down.
    #[test]
    fn requests_run_on_a_bounded_pool() {
        let (tx, rx) = mpsc::channel();
        spawn(|| panic!("a bad request"));
        let n = 64;
        for i in 0..n {
            let tx = tx.clone();
            spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(2));
                let _ = tx.send((i, std::thread::current().name().map(str::to_owned)));
            });
        }
        let mut threads = std::collections::HashSet::new();
        let mut done = std::collections::HashSet::new();
        for _ in 0..n {
            let (i, name) = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("a job never ran");
            done.insert(i);
            let name = name.expect("a pool thread");
            assert!(name.starts_with("qrtz-media-"), "{name}");
            threads.insert(name);
        }
        assert_eq!(done.len(), n);
        assert!(threads.len() <= WORKERS, "{threads:?}");
    }

    /// N140: playback follows the meeting rows. A restored recording under its renamed file plays; an orphan
    /// `<uuid>.wav` no row owns is 404; the owner's tombstone still hides its renamed file.
    #[test]
    fn a_recording_is_served_only_when_a_meeting_owns_it() {
        let data = tempfile::tempdir().unwrap();
        let rec = data.path().join("recordings");
        std::fs::create_dir(&rec).unwrap();
        let id = &ID[..36];
        let restored = format!("{id}-restored-1760745600000.wav");
        std::fs::write(rec.join(&restored), b"RIFFrestored").unwrap();
        std::fs::write(rec.join(ID), b"RIFForphan").unwrap(); // the clash the import moved aside of
        let rows = [(id.to_string(), restored.clone())];
        let owner = |name: &str| {
            rows.iter()
                .find(|(_, file)| file == name)
                .map(|(id, _)| id.clone())
        };
        let pending = PendingDeletes::new(data.path());
        let get = |name: &str| {
            let r = Request::builder()
                .uri(format!("qrtz-media://rec/{name}"))
                .body(Vec::new())
                .unwrap();
            super::serve(&rec, &pending, &owner, &r)
        };
        let r = get(&restored);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.body(), b"RIFFrestored");
        let orphan = get(ID);
        assert_eq!(orphan.status(), StatusCode::NOT_FOUND, "no row owns the orphan");
        assert!(orphan.body().is_empty());
        // A row whose file is missing is 404 too, not a 500.
        std::fs::rename(rec.join(&restored), rec.join("moved.wav")).unwrap();
        assert_eq!(get(&restored).status(), StatusCode::NOT_FOUND);
        std::fs::rename(rec.join("moved.wav"), rec.join(&restored)).unwrap();
        pending.add(id).unwrap();
        assert_eq!(get(&restored).status(), StatusCode::NOT_FOUND, "tombstoned owner");
        pending.remove(id).unwrap();
        assert_eq!(get(&restored).status(), StatusCode::OK);
        // The name rule still applies before any lookup.
        assert_eq!(get("..%2Fconfig.json").status(), StatusCode::FORBIDDEN);
    }
}
