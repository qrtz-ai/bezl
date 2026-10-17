//! A stand-in for the Swift qrtz-helper that speaks the same protocol, for Rust integration tests and Linux
//! dev runs of the Tauri build. Knobs (env):
//!   QRTZ_MOCK_HB_MS=200          heartbeat interval (0 = no heartbeats)
//!   QRTZ_MOCK_SILENT=1           never print anything (heartbeat-timeout path)
//!   QRTZ_MOCK_SILENT=system@1500,mic@2000-4000
//!                                N29: per recording, that source reports capture.silent 1500 ms after
//!                                recording.start (and capture.audible at 4000 ms); cancelled by stop/abort.
//!                                Same format as the TS mock (helper-process.ts parseSilence).
//!   QRTZ_MOCK_FALLBACK=screen>mic,mic>
//!                                H8: capture.fallback {from, to, reason} lines before every recording.started;
//!                                an empty `to` is null (no source left) and is followed by error audio.unavailable.
//!   QRTZ_MOCK_MIC_FAILS=1500     N35: per recording, the in-source mic fails 1500 ms after recording.start:
//!                                capture.degraded {degraded: ["no-mic"], reason}; scripted mic silence after it
//!                                is not sent; cancelled by stop/abort. Same knob as the TS mock (parseMicFails).
//!   QRTZ_MOCK_MIC_FAILS=start    N38: the mic fails at start; capture.degraded is written in the same write (one
//!                                stdout chunk) as recording.started, like Swift `DegradationInbox.goLive`.
//!   QRTZ_MOCK_EXIT_AFTER_MS=n    exit(3) after n ms
//!   QRTZ_MOCK_CRASH_ONCE=path    crash (exit 3) 100 ms after start unless `path` exists; creates it first
//!   QRTZ_MOCK_GARBAGE=1          print one invalid line right after hello
//!   QRTZ_MOCK_PERMISSIONS=microphone=denied,...
//!   speech.attach (ADR 0046 §1, N55): receives the audio write ends on fd QRTZ_SPEECH_FD, checks them like the
//!                                Swift helper (exact count, FIFO, write-only, session id) and writes 20 ms PCM16
//!                                frames to them while the named capture runs; closes them when it ends (EOF).
//!   QRTZ_MOCK_DICTATION="hello from qrtz talk"
//!                                dictation: `dictation.started`, then every 150 ms a level + a growing
//!                                `dictation.asr` partial; `dictation.stop` answers `dictation.stopped {text}`;
//!                                `insert.text` answers `insert.result` (ok, method ax-selected-text).
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

/// N29 script entry: (source, silent after ms, audible after ms).
type Silence = (String, u64, Option<u64>);

/// `system@1500,mic@2000-4000`; junk entries are dropped (like the TS parseSilence).
fn parse_silence(v: &str) -> Vec<Silence> {
    v.split(',')
        .filter_map(|kv| {
            let (src, rest) = kv.trim().split_once('@')?;
            if src != "system" && src != "mic" {
                return None;
            }
            let (a, b) = match rest.split_once('-') {
                Some((a, b)) => (a, Some(b.parse::<u64>().ok()?)),
                None => (rest, None),
            };
            Some((src.to_owned(), a.parse::<u64>().ok()?, b))
        })
        .collect()
}

/// `screen>mic,mic>` → [(from, to)], an empty `to` being None.
fn parse_fallback(v: &str) -> Vec<(String, Option<String>)> {
    v.split(',')
        .filter_map(|kv| {
            let (from, to) = kv.trim().split_once('>')?;
            (!from.is_empty()).then(|| (from.to_owned(), Some(to.to_owned()).filter(|t| !t.is_empty())))
        })
        .collect()
}

/// The helper's silence threshold (CAPTURE_SILENCE.afterMs in protocol.ts).
const SILENCE_AFTER_MS: u64 = 60_000;

fn main() {
    let silent_env = env("QRTZ_MOCK_SILENT");
    // Backward compatible: `1` still means a mute helper; anything else is the N29 silence script.
    let silent = silent_env.as_deref() == Some("1");
    let silence = silent_env
        .as_deref()
        .filter(|_| !silent)
        .map(parse_silence)
        .unwrap_or_default();
    let fallback = env("QRTZ_MOCK_FALLBACK")
        .as_deref()
        .map(parse_fallback)
        .unwrap_or_default();
    let mic_fails_env = env("QRTZ_MOCK_MIC_FAILS");
    let mic_fails_at_start = mic_fails_env.as_deref().map(str::trim) == Some("start");
    let mic_fails = mic_fails_env.and_then(|v| v.trim().parse::<u64>().ok());
    // The generation whose mic failed (0 = none): the mic is no longer tracked then.
    let mic_failed = Arc::new(AtomicU64::new(0));
    // Bumped by every recording.start / stop / abort; a scripted line only goes out while its generation is current.
    let generation = Arc::new(AtomicU64::new(0));
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let say = {
        let out = out.clone();
        move |v: Value| {
            if silent {
                return;
            }
            let mut o = out.lock().unwrap();
            let _ = writeln!(o, "{}", serde_json::to_string(&v).unwrap());
            let _ = o.flush();
        }
    };
    let mut perms =
        json!({"accessibility": "granted", "microphone": "granted", "screen-recording": "granted"});
    for kv in env("QRTZ_MOCK_PERMISSIONS").unwrap_or_default().split(',') {
        if let Some((k, v)) = kv.split_once('=') {
            perms[k.trim()] = json!(v.trim());
        }
    }

    say(
        json!({"type": "hello", "protocol": 1, "helperVersion": "0.0.0-mock", "os": "mock", "features": ["mock"]}),
    );
    if env("QRTZ_MOCK_GARBAGE").as_deref() == Some("1") && !silent {
        println!("{{not json");
    }
    if let Some(p) = env("QRTZ_MOCK_CRASH_ONCE") {
        if !std::path::Path::new(&p).exists() {
            std::fs::write(&p, b"crashed").unwrap();
            std::thread::sleep(Duration::from_millis(100));
            std::process::exit(3);
        }
    }
    if let Some(ms) = env("QRTZ_MOCK_EXIT_AFTER_MS").and_then(|v| v.parse::<u64>().ok()) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            std::process::exit(3);
        });
    }
    let hb = env("QRTZ_MOCK_HB_MS")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(200);
    if hb > 0 {
        let say = say.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(hb));
            say(json!({"type": "heartbeat", "ts": now()}));
        });
    }

    let mut audio: Option<String> = None;

    let dictation_words: Arc<Vec<String>> = Arc::new(
        env("QRTZ_MOCK_DICTATION")
            .unwrap_or_else(|| "hello from qrtz talk".into())
            .split_whitespace()
            .map(String::from)
            .collect(),
    );

    let dictation_gen = Arc::new(AtomicU64::new(0));

    let dictation_heard = Arc::new(AtomicU64::new(0));

    let mut dictation_id: Option<String> = None;

    let live_targets: Arc<Mutex<std::collections::HashSet<String>>> = Default::default();
    let mut started = 0u64;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match msg["type"].as_str().unwrap_or_default() {
            "ping" => say(json!({"type": "pong", "id": msg["id"]})),
            "permissions.check" => say(json!({"type": "permissions.status", "status": perms})),
            "permissions.request" => say(json!({"type": "permissions.status", "status": perms})),
            "recording.start" => {
                let path = msg["audioPath"].as_str().unwrap_or_default().to_owned();
                started = now();
                let gen = generation.fetch_add(1, Ordering::SeqCst) + 1;
                for (from, to) in &fallback {
                    say(
                        json!({"type": "capture.fallback", "from": from, "to": to, "reason": "mock: source failed to start"}),
                    );
                }
                if fallback.iter().any(|(_, to)| to.is_none()) {
                    say(
                        json!({"type": "error", "code": "audio.unavailable", "message": "no audio source could start", "fatal": false}),
                    );
                }
                for (source, after, audible) in silence.clone() {
                    let (say, generation, mic_failed) = (say.clone(), generation.clone(), mic_failed.clone());
                    let is_mic = source == "mic";
                    let gone = move || is_mic && mic_failed.load(Ordering::SeqCst) == gen;
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(after));
                        if generation.load(Ordering::SeqCst) != gen || gone() {
                            return;
                        }
                        let since = (now() - started).saturating_sub(SILENCE_AFTER_MS);
                        say(json!({"type": "capture.silent", "source": source, "sinceMs": since}));
                        let Some(audible) = audible else { return };
                        std::thread::sleep(Duration::from_millis(audible.saturating_sub(after)));
                        if generation.load(Ordering::SeqCst) == gen && !gone() {
                            say(json!({"type": "capture.audible", "source": source}));
                        }
                    });
                }
                if mic_fails_at_start {
                    // N38: one write, two lines, so the host sees them in a single read chunk.
                    mic_failed.store(gen, Ordering::SeqCst);
                    if !silent {
                        let started = json!({"type": "recording.started", "audioPath": path});
                        let degraded = json!({"type": "capture.degraded", "degraded": ["no-mic"], "reason": "mock: mic device failed"});
                        let mut o = out.lock().unwrap();
                        let _ = o.write_all(format!("{started}\n{degraded}\n").as_bytes());
                        let _ = o.flush();
                    }
                } else {
                    say(json!({"type": "recording.started", "audioPath": path}));
                }
                // N35: like the Swift controller, only after recording.started, once per recording, never after
                // stop/abort (the generation moves on).
                if let Some(after) = mic_fails {
                    let (say, generation, mic_failed) = (say.clone(), generation.clone(), mic_failed.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(after));
                        if generation.load(Ordering::SeqCst) == gen {
                            mic_failed.store(gen, Ordering::SeqCst);
                            say(
                                json!({"type": "capture.degraded", "degraded": ["no-mic"], "reason": "mock: mic device failed"}),
                            );
                        }
                    });
                }
                say(
                    json!({"type": "asr", "event": {"kind": "partial", "utteranceId": "u1", "text": "hello", "startMs": 0}}),
                );
                say(
                    json!({"type": "asr", "event": {"kind": "final", "utteranceId": "u1", "text": "Hello from the mock helper.", "startMs": 0, "endMs": 1200}}),
                );
                live_targets.lock().unwrap().insert(format!("r:{path}"));
                audio = Some(path);
            }
            "recording.stop" => {
                generation.fetch_add(1, Ordering::SeqCst);
                if let Some(p) = &audio {
                    live_targets.lock().unwrap().remove(&format!("r:{p}"));
                }
                if let Some(p) = audio.take() {
                    say(json!({"type": "recording.stopped", "audioPath": p, "durationMs": now() - started}));
                }
            }
            "recording.abort" => {
                // N2: scoped teardown without a flush; answered even when nothing records that file.
                let p = msg["audioPath"].as_str().unwrap_or_default().to_owned();
                if audio.as_deref() == Some(p.as_str()) {
                    audio = None;
                    generation.fetch_add(1, Ordering::SeqCst);
                }
                live_targets.lock().unwrap().remove(&format!("r:{p}"));
                say(json!({"type": "recording.aborted", "audioPath": p}));
            }
            "speech.attach" => {
                let sid = msg["sessionId"].as_str().unwrap_or_default().to_owned();
                let n = msg["channels"].as_array().map_or(0, Vec::len);
                let key = match msg["target"]["kind"].as_str() {
                    Some("dictation") => format!("d:{}", msg["target"]["id"].as_str().unwrap_or_default()),
                    _ => format!("r:{}", msg["target"]["audioPath"].as_str().unwrap_or_default()),
                };
                let fd = env(qrtz_helper_bridge::fdpass::SPEECH_FD_ENV).and_then(|v| v.parse::<i32>().ok());
                let r = match fd {
                    None => Err("unsupported"),
                    Some(fd) => {
                        qrtz_helper_bridge::fdpass::accept_attach(fd, &sid, n, Duration::from_secs(2))
                    }
                };
                match r {
                    Err(reason) => {
                        say(json!({"type": "speech.attach.rejected", "sessionId": sid, "reason": reason}))
                    }
                    Ok(fds) => {
                        say(json!({"type": "speech.attached", "sessionId": sid}));
                        let live = live_targets.clone();
                        std::thread::spawn(move || write_audio(fds, key, live));
                    }
                }
            }
            "dictation.configure" => {}
            "dictation.start" => {
                let id = msg["id"].as_str().unwrap_or_default().to_owned();
                let gen = dictation_gen.fetch_add(1, Ordering::SeqCst) + 1;
                say(json!({"type": "dictation.started", "id": id, "preRollMs": 0}));
                let (say, dg, heard) = (say.clone(), dictation_gen.clone(), dictation_heard.clone());
                let words = dictation_words.clone();
                heard.store(0, Ordering::SeqCst);
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(150));
                    if dg.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    let n = (heard.fetch_add(1, Ordering::SeqCst) + 1).min(words.len() as u64) as usize;
                    say(json!({"type": "dictation.level", "rms": 0.2}));
                    say(
                        json!({"type": "dictation.asr", "id": id, "event": {"kind": "partial", "utteranceId": "u1",
                                "startMs": 0, "text": words[..n].join(" ")}}),
                    );
                });
                dictation_id = Some(msg["id"].as_str().unwrap_or_default().to_owned());
                live_targets
                    .lock()
                    .unwrap()
                    .insert(format!("d:{}", msg["id"].as_str().unwrap_or_default()));
            }
            ty @ ("dictation.stop" | "dictation.cancel") => {
                let id = msg["id"].as_str().unwrap_or_default().to_owned();
                if dictation_id.as_deref() == Some(id.as_str()) {
                    dictation_gen.fetch_add(1, Ordering::SeqCst);
                    dictation_id = None;
                    live_targets.lock().unwrap().remove(&format!("d:{id}"));
                    if ty == "dictation.stop" {
                        let n = dictation_heard
                            .load(Ordering::SeqCst)
                            .clamp(1, dictation_words.len() as u64) as usize;
                        say(
                            json!({"type": "dictation.stopped", "id": id, "text": dictation_words[..n].join(" "), "durationMs": 1000}),
                        );
                    }
                }
            }
            "insert.text" => say(
                json!({"type": "insert.result", "id": msg["id"], "method": "ax-selected-text", "ok": true, "app": "Mock Editor"}),
            ),
            "recording.pause" | "recording.resume" => {}
            "shutdown" => std::process::exit(0),
            _ => say(
                json!({"type": "error", "code": "protocol.unknown", "message": "unknown message", "fatal": false}),
            ),
        }
    }
}

/// Write 20 ms PCM16 frames (a quiet tone) to the attached fds while `key` is live; the fds are O_NONBLOCK (the
/// host set them, N77): a full pipe drops the frame, a closed reader ends the writer. Dropping the fds is the EOF.
fn write_audio(
    fds: Vec<std::os::fd::OwnedFd>,
    key: String,
    live: Arc<Mutex<std::collections::HashSet<String>>>,
) {
    use std::os::fd::AsRawFd;
    let frame: Vec<u8> = (0..320)
        .flat_map(|i| (((i as f32 * 0.1).sin() * 800.0) as i16).to_le_bytes())
        .collect();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut seen = false;
    loop {
        std::thread::sleep(Duration::from_millis(20));
        let on = live.lock().unwrap().contains(&key);
        if on {
            seen = true;
        } else if seen || std::time::Instant::now() > deadline {
            return;
        } else {
            continue;
        }
        for f in &fds {
            // SAFETY: valid fd and buffer.
            let n = unsafe { libc::write(f.as_raw_fd(), frame.as_ptr().cast(), frame.len()) };
            if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EPIPE) {
                return;
            }
        }
    }
}
