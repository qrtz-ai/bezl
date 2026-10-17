//! `qrtz-media://rec/<uuid>.wav`: recordings for the renderer's <audio>, nothing else (same rule as the
//! Electron `protocol.handle('qrtz-media')`). Supports single byte ranges so WebKit can seek.
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
    let range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|h| parse_range(h, len));
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
}
