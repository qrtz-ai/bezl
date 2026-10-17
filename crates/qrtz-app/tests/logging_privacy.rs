//! ADR 0083, the confinement requirement: the local log never holds what the user said, dictated or wrote.
//!
//! The app's own logging (`qrtz_app::logging::init`: the redacting stderr + rotating file sinks) at `trace` for
//! every target, over a scripted meeting (live transcript, a live note, a speaker rename, the on-device summary,
//! markdown export) and a dictation (dictionary, insert, copy to the clipboard), plus the failure paths that used
//! to quote content (a malformed helper line, an invalid event, an invalid host message). Then every piece of text
//! the run produced, and fixed sentinels, must be absent from the log file, while the log itself is clearly
//! working (startup line, helper/renderer lines, export).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const NOTE: &str = "sentinel note: the zebra quartet signs the lease on thursday";
const SPEAKER: &str = "Sentinel Speaker Okonkwo";
const ALIAS: &str = "zebra quartet alias";
const BAD_LINE: &str = "sentinel helper line about the secret merger with globex";
const BAD_EVENT: &str = "sentinel transcript delta that must stay out of the log";
const PROMPT: &str = "sentinel summary prompt quoting the whole meeting transcript";
const HOME_FILE: &str = "sentinel-home-file.wav";

/// The process-wide log, initialised once for this test binary like `apps/desktop/src-tauri` does at launch.
fn log_dir() -> &'static PathBuf {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("qrtz-log-privacy-{}", std::process::id()));
        qrtz_app::logging::init(qrtz_app::logging::LogOptions {
            dir: Some(d.clone()),
            release: false,
            env_filter: Some("trace".into()),
            version: "0.0.0-test".into(),
            build: "debug test".into(),
        });
        d
    })
}

fn log_text() -> String {
    qrtz_app::logging::flush();
    qrtz_app::logging::log_files(log_dir())
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap_or_default())
        .collect()
}

/// Every string in `v` that reads like text a person wrote or said (has a space, long enough to be specific).
fn texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let s = s.trim();
            if s.len() >= 14 && s.contains(' ') && !s.contains('/') {
                out.push(s.to_owned());
            }
        }
        Value::Array(a) => a.iter().for_each(|x| texts(x, out)),
        Value::Object(o) => o.values().for_each(|x| texts(x, out)),
        _ => {}
    }
}

fn assert_absent(log: &str, what: &str, needles: &[String]) {
    assert!(!needles.is_empty(), "{what}: nothing to look for");
    for n in needles {
        assert!(!log.contains(n.as_str()), "{what} leaked into the log: {n:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_transcript_dictation_note_summary_or_title_reaches_the_log() {
    let dir = log_dir().clone();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default().standup("0.02")).await;
    a.helper_ready().await;

    // --- a meeting: live transcript, a live note, the summary, a speaker rename, markdown export.
    a.until_phase("detected").await;
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    a.until_phase("recording").await;
    a.note("note:add-note", json!({ "text": NOTE })).await;
    wait("live transcript", || async {
        let t = a.note("note:live-transcript", json!({})).await;
        (segments(&t) > 5).then_some(())
    })
    .await;
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    a.until_settled(&id).await;
    let d = a.note("note:get", json!({ "id": id })).await;
    let speaker = d["transcript"]
        .as_array()
        .and_then(|t| t.iter().find_map(|s| s["speaker"].as_str()))
        .map(str::to_owned);
    if let Some(from) = speaker {
        a.note(
            "note:rename-speaker",
            json!({"meetingId": id, "from": from, "to": SPEAKER}),
        )
        .await;
    }
    let md = a.note("note:export", json!({ "id": id })).await;

    // --- a dictation: dictionary, streaming text, insert, history, copy to the clipboard.
    let b = App::launch(&tmp.path().join("talk"), Opts::default().helper(Helper::Scripted)).await;
    b.helper_ready().await;
    b.talk(
        "talk:set-dictionary",
        json!({"entries": [{"term": "qrtz", "aliases": ["cue arts", ALIAS]}]}),
    )
    .await;
    let e = b.dictate().await;
    b.talk("talk:copy", json!({ "id": e["id"] })).await;

    // --- the failure paths that used to quote what they failed on.
    let svc = a.app.service.clone();
    svc.on_helper_line(
        &json!({"type": "asr", "event": {"kind": "bogus", "utteranceId": "u", "text": BAD_LINE}}).to_string(),
    );
    svc.on_helper_line(&format!("{{\"type\":\"dictation.asr\",\"text\":\"{BAD_LINE}\""));
    svc.events.emit(
        "note:transcript",
        json!({ "meetingId": id, "append": [{ "text": BAD_EVENT }] }),
    );
    let enc = qrtz_helper_bridge::encode_host_message(&json!({"type": "summarize", "prompt": PROMPT}));
    assert!(!enc.unwrap_err().contains(PROMPT));
    // The helper's stderr lands under the `helper` target, one event per line, cut at STDERR_LINE_MAX, escaped.
    let long = "z".repeat(qrtz_helper_bridge::process::STDERR_LINE_MAX + 500);
    let stderr = format!("[qrtz-helper] nemotron: model load failed\n{long}\nescape \u{1b}[31m red\r\n");
    qrtz_helper_bridge::process::log_stderr(stderr.as_bytes()).await;
    // A helper `log` protocol line too.
    svc.on_helper_line(r#"{"type":"log","level":"warn","msg":"diarizer mic fell 30 s behind"}"#);
    // A path in the user's home is shortened to `~`.
    if let Some(home) = qrtz_app::logging::home_dir() {
        tracing::warn!(path = %home.join(HOME_FILE).display(), "a path under home");
    }

    // --- the renderer's channel works, is capped and rate-limited; export copies the folder.
    let w = a
        .call(
            "shell",
            "log:write",
            json!({"level": "error", "scope": "player", "message": "media error 4 (src-not-supported)"}),
        )
        .await;
    assert_eq!(w["accepted"], true);
    let over = a
        .raw(
            "shell",
            "log:write",
            json!({"level": "info", "scope": "x", "message": "y".repeat(1001)}),
        )
        .await;
    assert_eq!(over["error"]["code"], "E_VALIDATION", "{over}");
    let forbidden = a
        .raw(
            "note",
            "log:write",
            json!({"level": "info", "scope": "x", "message": "m"}),
        )
        .await;
    assert_eq!(forbidden["error"]["code"], "E_FORBIDDEN", "{forbidden}");
    let mut accepted = 1;
    for _ in 0..100 {
        let r = a
            .call(
                "shell",
                "log:write",
                json!({"level": "debug", "scope": "flood", "message": "x"}),
            )
            .await;
        accepted += r["accepted"].as_bool().unwrap() as u32;
    }
    assert_eq!(accepted, qrtz_app::logging::RENDERER_LINES_PER_WINDOW);

    let out = tmp.path().join("exported");
    std::fs::create_dir_all(&out).unwrap();
    *a.shell.logs_to.lock().unwrap() = Some(out.clone());
    let x = a.settings("logs:export", json!({})).await;
    let saved = PathBuf::from(x["savedTo"].as_str().unwrap());
    assert!(x["files"].as_u64().unwrap() >= 1, "{x}");
    let exported = std::fs::read_to_string(&saved).unwrap();
    assert!(
        exported.starts_with("qrtz 0.1.0 logs"),
        "{}",
        &exported[..80.min(exported.len())]
    );
    assert!(exported.contains("qrtz started"));
    let r = a.settings("logs:export", json!({ "reveal": true })).await;
    assert_eq!(r["revealed"], true);
    assert_eq!(*a.shell.revealed.lock().unwrap(), vec![dir.clone()]);
    // Note's identity can't export logs.
    let no = a.raw("note", "logs:export", json!({})).await;
    assert_eq!(no["error"]["code"], "E_FORBIDDEN", "{no}");

    a.quit().await;
    b.quit().await;

    // --- the log is working…
    let log = log_text();
    for want in [
        "qrtz started",
        "recording started",
        "media error 4 (src-not-supported)",
        "bad helper line",
        "dropping invalid event",
        "talk latency",
        "helper: [qrtz-helper] nemotron: model load failed",
        "helper: diarizer mic fell 30 s behind",
        "escape ",
    ] {
        assert!(log.contains(want), "expected {want:?} in the log");
    }
    assert!(
        !log.contains('\u{1b}') && !log.contains('\r'),
        "control characters reach the file escaped"
    );
    let cut = log
        .lines()
        .find(|l| l.contains("zzzz"))
        .expect("the long stderr line");
    assert!(cut.contains("cut=500"), "{}", &cut[..120]);
    assert!(!cut.contains(&long), "a stderr line is cut");
    if qrtz_app::logging::home_dir().is_some() {
        assert!(log.contains(&format!("~/{HOME_FILE}")), "home not shortened");
    }
    // …and holds none of it.
    let mut meeting = Vec::new();
    texts(&d, &mut meeting);
    texts(&a_note_list(&md), &mut meeting);
    assert!(
        meeting.iter().any(|t| t.contains("zebra quartet")),
        "the note is in the meeting"
    );
    assert_absent(&log, "meeting text", &meeting);
    let mut talk = Vec::new();
    texts(&e, &mut talk);
    talk.push(e["text"].as_str().unwrap().to_owned());
    assert_absent(&log, "dictated text", &talk);
    let fixed: Vec<String> = [
        NOTE,
        SPEAKER,
        ALIAS,
        BAD_LINE,
        BAD_EVENT,
        PROMPT,
        "Weekly Product Sync",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_absent(&log, "sentinel", &fixed);
    assert!(!log.contains("cue arts"), "dictionary alias in the log");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let f = dir.join(qrtz_app::logging::LOG_FILE);
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let _ = std::fs::remove_dir_all(Path::new(&dir));
}

/// The exported markdown, split into lines, as JSON strings for `texts`.
fn a_note_list(md: &Value) -> Value {
    Value::Array(
        md["markdown"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .map(|l| Value::String(l.trim_start_matches(['#', '-', '*', ' ', '>']).to_owned()))
            .collect(),
    )
}

/// Field names that would carry what the user said or wrote. A `tracing` call in any crate's sources (or the shell)
/// that names one, as `name = …`, the `%name` / `?name` shorthand or an inline `{name}` capture in a message
/// (N136; only [`INLINE_ALLOWED`]), fails here: log its length or id instead.
const CONTENT_FIELDS: &[&str] = &[
    "text",
    "transcript",
    "body",
    "title",
    "prompt",
    "summary",
    "markdown",
    "clipboard",
    "dictionary",
    "content",
    "words",
    "partial",
    "query",
    "notes",
];

fn strip_strings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut esc = false;
    for c in s.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                out.push('"');
            }
        } else {
            if c == '"' {
                in_str = true;
            }
            out.push(c);
        }
    }
    out
}

fn content_fields_in(call: &str) -> Vec<String> {
    let code = strip_strings(call);
    let b = code.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut hits = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if ident(b[i]) && (i == 0 || !ident(b[i - 1]) && b[i - 1] != b'.') {
            let start = i;
            while i < b.len() && ident(b[i]) {
                i += 1;
            }
            let word = &code[start..i];
            if CONTENT_FIELDS.contains(&word) {
                let prev = code[..start].trim_end().bytes().last();
                let next = code[i..].trim_start();
                let field = next.starts_with('=') && !next.starts_with("==");
                let shorthand = matches!(prev, Some(b'%' | b'?'));
                if field || shorthand {
                    hits.push(word.to_owned());
                }
            }
        } else {
            i += 1;
        }
    }
    hits
}

/// The string literals of a call, unescaped only as far as needed to find `{…}` (N136).
fn string_literals(call: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut esc = false;
    for c in call.chars() {
        match cur.as_mut() {
            Some(lit) => {
                if esc {
                    esc = false;
                    lit.push(c);
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    out.push(cur.take().unwrap());
                } else {
                    lit.push(c);
                }
            }
            None if c == '"' => cur = Some(String::new()),
            None => {}
        }
    }
    out
}

/// N136: inline captures in a message (`"{title}"`, `"{text:?}"`, `"{ prompt }"`) name a content field too;
/// `{{` is a literal brace.
fn inline_content_in(call: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for lit in string_literals(call) {
        let mut rest = lit.as_str();
        while let Some(at) = rest.find('{') {
            if rest[at..].starts_with("{{") {
                rest = &rest[at + 2..];
                continue;
            }
            let inner = &rest[at + 1..];
            let Some(close) = inner.find('}') else { break };
            let name = inner[..close].split(':').next().unwrap_or_default().trim();
            if CONTENT_FIELDS.contains(&name) {
                hits.push(name.to_owned());
            }
            rest = &inner[close..];
        }
    }
    hits
}

/// The one inline capture allowed (N136): the helper's stderr pass-through, cut to 2000 chars and redacted by the
/// log layer (`qrtz-helper-bridge` `process.rs` `emit_stderr_line`).
const INLINE_ALLOWED: &[(&str, &str)] = &[("qrtz-helper-bridge/src/process.rs", "text")];

#[test]
fn no_tracing_call_names_a_content_field() {
    assert_eq!(inline_content_in(r#"tracing::warn!("bad {title} x")"#), ["title"]);
    assert_eq!(
        inline_content_in(r#"tracing::warn!(n = 1, "{text:?} and { prompt } and {{body}} and {id}")"#),
        ["text", "prompt"]
    );
    assert!(inline_content_in(r#"tracing::info!(len = text.len(), "{len} chars")"#).is_empty());
    assert_eq!(content_fields_in(r#"tracing::info!(text = %t, "x")"#), ["text"]);
    assert_eq!(
        content_fields_in(r#"tracing::info!(%title, "a title = b")"#),
        ["title"]
    );
    assert!(content_fields_in(r#"tracing::info!(len = text.len(), "text")"#).is_empty());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .unwrap()
        .map(|e| e.unwrap().path().join("src"))
        .collect();
    dirs.push(root.join("apps/desktop/src-tauri/src"));
    let mut bad = Vec::new();
    let mut seen = 0;
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            for mac in ["trace!(", "debug!(", "info!(", "warn!(", "error!(", "event!("] {
                let mut from = 0;
                while let Some(at) = src[from..].find(mac) {
                    let open = from + at + mac.len();
                    let mut depth = 1;
                    let mut end = open;
                    for (j, c) in src[open..].char_indices() {
                        match c {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        if depth == 0 {
                            end = open + j;
                            break;
                        }
                    }
                    seen += 1;
                    let mut hits = content_fields_in(&src[open..end]);
                    let unix = p.to_string_lossy().replace('\\', "/");
                    hits.extend(inline_content_in(&src[open..end]).into_iter().filter(|h| {
                        !INLINE_ALLOWED
                            .iter()
                            .any(|(file, name)| unix.ends_with(file) && h == name)
                    }));
                    if !hits.is_empty() {
                        let line = src[..open].lines().count();
                        bad.push(format!("{}:{line}: {hits:?}", p.display()));
                    }
                    from = end.max(open);
                }
            }
        }
    }
    assert!(seen > 100, "found only {seen} tracing calls");
    assert!(
        bad.is_empty(),
        "tracing calls naming content fields (as a field, %/? shorthand or an inline {{capture}}):\n{}",
        bad.join("\n")
    );
}
