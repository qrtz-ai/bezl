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
//!   QRTZ_MOCK_EXIT_AFTER_MS=n    exit(3) after n ms
//!   QRTZ_MOCK_CRASH_ONCE=path    crash (exit 3) 100 ms after start unless `path` exists; creates it first
//!   QRTZ_MOCK_GARBAGE=1          print one invalid line right after hello
//!   QRTZ_MOCK_PERMISSIONS=microphone=denied,...
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
    let mic_fails = env("QRTZ_MOCK_MIC_FAILS").and_then(|v| v.trim().parse::<u64>().ok());
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
                say(json!({"type": "recording.started", "audioPath": path}));
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
                audio = Some(path);
            }
            "recording.stop" => {
                generation.fetch_add(1, Ordering::SeqCst);
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
                say(json!({"type": "recording.aborted", "audioPath": p}));
            }
            "recording.pause" | "recording.resume" => {}
            "shutdown" => std::process::exit(0),
            _ => say(
                json!({"type": "error", "code": "protocol.unknown", "message": "unknown message", "fatal": false}),
            ),
        }
    }
}
