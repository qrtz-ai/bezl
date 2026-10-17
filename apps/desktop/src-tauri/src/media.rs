//! `qrtz-media://rec/<uuid>.wav`: recordings for the renderer's <audio>, nothing else (same rule as the
//! Electron `protocol.handle('qrtz-media')`). Supports single byte ranges so WebKit can seek.
//!
//! A Tauri protocol response is one in-memory body, so no response is larger than [`MAX_CHUNK`]: a request
//! with no Range header for a bigger file gets `206` with the first chunk, and a range (WebKit asks for
//! `bytes=0-`) is cut to a chunk from its start. The player reads `Content-Range` and asks for the rest as it
//! plays or seeks, so memory stays bounded however long the recording is (a 2 h WAV is about 230 MB).
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tauri::http::{header, Request, Response, StatusCode};

pub fn valid_name(name: &str) -> bool {
    name.len() == 40
        && name.ends_with(".wav")
        && name[..36]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
}

/// Parses `bytes=a-b` / `bytes=a-` / `bytes=-n` against `len` into an inclusive range.
pub fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (len.saturating_sub(n), len - 1)
        }
        (s, "") => (s.parse().ok()?, len - 1),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(len - 1)),
    };
    (start <= end && start < len).then_some((start, end))
}

/// The most bytes one response carries.
pub const MAX_CHUNK: u64 = 4 * 1024 * 1024;

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).unwrap()
}

pub fn serve(recordings: &Path, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    // macOS/Linux: qrtz-media://rec/<name>; Windows: http://qrtz-media.localhost/rec/<name>.
    let path = req.uri().path().trim_start_matches('/');
    let name = path.strip_prefix("rec/").unwrap_or(path);
    let name = name.replace("%2D", "-");
    if !valid_name(&name) {
        return status(StatusCode::FORBIDDEN);
    }
    let Ok(mut f) = std::fs::File::open(recordings.join(&name)) else {
        return status(StatusCode::NOT_FOUND);
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    serve_file(&mut f, len, req, MAX_CHUNK)
}

/// [`serve`] once the file is open; `chunk` is the response size cap ([`MAX_CHUNK`]; tests pass a small one).
fn serve_file(f: &mut std::fs::File, len: u64, req: &Request<Vec<u8>>, chunk: u64) -> Response<Vec<u8>> {
    let mut range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|h| parse_range(h, len));
    if range.is_none() && len > chunk {
        // No Range on a file over the cap: answer the first chunk as a partial response.
        range = Some(Some((0, len - 1)));
    }
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

    const ID: &str = "0b9f6a2e-6c1d-4e7a-9a43-2f1d0c8e5b71.wav";

    #[test]
    fn only_uuid_wavs() {
        assert!(valid_name(ID));
        for bad in [
            "../config.json",
            "x.wav",
            "0B9F6A2E-6C1D-4E7A-9A43-2F1D0C8E5B71.wav",
            &ID.replace(".wav", ".mp3"),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 10), Some((0, 9)));
        assert_eq!(parse_range("bytes=2-4", 10), Some((2, 4)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=5-99", 10), Some((5, 9)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
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
}
