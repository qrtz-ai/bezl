//! Test-only scripted capture helper for the phase 5 E2E port (`crates/qrtz-app/tests/e2e_*.rs`, docs/e2e-parity.md).
//!
//! It speaks the helper protocol like the TS mock the Electron Playwright suite runs against
//! (`packages/core/src/helper/mock.ts` + `apps/desktop/src/main/helper-process.ts`), for the scenarios the Rust
//! `qrtz-mock-helper` has no knob for yet. Knobs (env), same names and formats as the TS mock:
//!   QRTZ_MOCK_SCENARIO=standup   AX detection (`ax.participants` + `meeting.detected` 300 ms after hello) and, per
//!                                recording, the scripted standup (partials → finals, `ax.activeSpeaker`, levels) and
//!                                the fixture WAV at the audio path (16 kHz mono PCM16, 36.5 s)
//!   QRTZ_MOCK_SPEED=0.05         time scale of the scenario
//!   QRTZ_MOCK_PERMISSIONS=microphone=denied,...
//!   QRTZ_MOCK_REVOKE=screen-recording@800,microphone@2000
//!                                H5: `permission.revoked` (with the arbiter's degradations) n ms after the first capture
//!   QRTZ_MOCK_INSERT=ax|paste|fail
//!   QRTZ_MOCK_CRASH_AFTER_MS=900 + QRTZ_MOCK_CRASH_MARK=<file>
//!                                once per mark file: n ms after recording.start, leave a half WAV with an unfinalized
//!                                header and exit(1) (the TS mock's once-per-app-run crash)
//!   QRTZ_MOCK_DICTATION_STEP_MS=60
//!   QRTZ_MOCK_DICTATION_START_MS=400   a slow engine start: `dictation.started` (and the words) that much later
//!   QRTZ_MOCK_INSERT_DELAY_MS=1500     a slow paste: `insert.result` that much after `insert.text`
//!   QRTZ_MOCK_CMD_LOG=<file>           one line per host message, `<type> <id>` (what reached the helper, in order)
//!   QRTZ_MOCK_HB_MS=200
//!   QRTZ_MOCK_SUMMARIZE / QRTZ_MOCK_SUMMARIZE_LOG  ADR 0075 on-device summaries (`qrtz-helper-bridge/mock/summarize.rs`)
#[path = "../../qrtz-helper-bridge/mock/summarize.rs"]
mod mock_summarize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DICTATION: &str = "um so I I think we should ship cue arts talk on friday";
const FIXTURE_DURATION_MS: u64 = 36_500;
const PARTICIPANTS: [&str; 3] = ["Ada Lovelace", "Grace Hopper", "Alan Turing"];
/// [speaker, startMs, endMs, text]: STANDUP_FIXTURE (packages/core/src/note/fixtures.ts).
const LINES: [(&str, u64, u64, &str); 8] = [
    (
        "Ada Lovelace",
        0,
        4200,
        "Welcome everyone, this is our weekly product sync for the qrtz desktop launch.",
    ),
    (
        "Grace Hopper",
        4400,
        9800,
        "The note recorder now captures system audio and the live transcript runs on the neural engine.",
    ),
    (
        "Ada Lovelace",
        10000,
        13500,
        "Great. What is the status of the summary pipeline?",
    ),
    (
        "Alan Turing",
        13800,
        19500,
        "The summary pipeline works offline with the extractive fallback and supports any LLM provider.",
    ),
    (
        "Ada Lovelace",
        19800,
        24000,
        "Grace, can you write the onboarding copy for accessibility permissions by Friday?",
    ),
    (
        "Grace Hopper",
        24300,
        27800,
        "Yes, I'll draft the onboarding copy tomorrow.",
    ),
    (
        "Alan Turing",
        28000,
        33000,
        "I will benchmark parakeet streaming latency on the M4 before the launch.",
    ),
    (
        "Ada Lovelace",
        33200,
        36000,
        "Perfect, let's ship the desktop launch next week. Thanks all.",
    ),
];

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

#[derive(Clone)]
struct Out(Arc<Mutex<std::io::Stdout>>);
impl Out {
    fn say(&self, v: Value) {
        let mut o = self.0.lock().unwrap();
        let _ = writeln!(o, "{v}");
        let _ = o.flush();
    }
}

/// The scripted standup as timed (ms, line) events: partials spread before each final (fixtureAsrEvents + mock.ts).
fn script() -> Vec<(u64, Value)> {
    let mut out = Vec::new();
    for (i, (who, start, end, text)) in LINES.iter().enumerate() {
        let words: Vec<&str> = text.split(' ').collect();
        let id = format!("u{i}");
        let partials: Vec<usize> = (3..words.len()).step_by(4).collect();
        let n = partials.len() as f64;
        for (j, k) in partials.iter().enumerate() {
            let t = *start as f64 + (*end - *start) as f64 * (j + 1) as f64 / (n + 1.5);
            out.push((
                t as u64,
                json!({"type": "asr", "event": {"kind": "partial", "utteranceId": id, "startMs": start,
                        "text": words[..*k].join(" ")}}),
            ));
        }
        out.push((
            *end,
            json!({"type": "asr", "event": {"kind": "final", "utteranceId": id, "startMs": start, "endMs": end,
                    "text": text, "confidence": 0.9}}),
        ));
        out.push((
            start + 250,
            json!({"type": "ax.activeSpeaker", "atMs": start + 250, "name": who}),
        ));
    }
    let mut t = 0;
    while t < FIXTURE_DURATION_MS {
        out.push((
            t,
            json!({"type": "audio.level", "rms": 0.2 + 0.6 * ((t as f64) / 700.0).sin().abs()}),
        ));
        t += 500;
    }
    out.sort_by_key(|(t, _)| *t);
    out
}

/// fixture-audio.ts writeFixtureWav: one tone per speaker, `finalized = false` leaves the header sizes at 0.
fn fixture_wav(path: &str, duration_ms: u64, finalized: bool) -> Vec<u8> {
    let rate = 16_000u64;
    let n = duration_ms * rate / 1000;
    let mut b = Vec::with_capacity(44 + n as usize * 2);
    let size = |x: u64| if finalized { x as u32 } else { 0 };
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&size(36 + n * 2).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(rate as u32).to_le_bytes());
    b.extend_from_slice(&(rate as u32 * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&size(n * 2).to_le_bytes());
    let mut pcm = vec![0i16; n as usize];
    for (who, start, end, _) in LINES {
        let f = 180.0 + 70.0 * PARTICIPANTS.iter().position(|p| *p == who).unwrap() as f64;
        let (a, z) = (
            (start * rate / 1000) as usize,
            ((end * rate / 1000) as usize).min(n as usize),
        );
        for (i, s) in pcm.iter_mut().enumerate().take(z).skip(a) {
            let env = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (rate as f64 * 0.3)).sin();
            *s = (env * 6000.0 * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()) as i16;
        }
    }
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    let _ = path;
    b
}

fn main() {
    let out = Out(Arc::new(Mutex::new(std::io::stdout())));
    let scenario = env("QRTZ_MOCK_SCENARIO").as_deref() == Some("standup");
    let speed: f64 = env("QRTZ_MOCK_SPEED")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let insert = env("QRTZ_MOCK_INSERT").unwrap_or_else(|| "ax".into());
    let step_ms: u64 = env("QRTZ_MOCK_DICTATION_STEP_MS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let start_ms: u64 = env("QRTZ_MOCK_DICTATION_START_MS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let insert_delay_ms: u64 = env("QRTZ_MOCK_INSERT_DELAY_MS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let cmd_log = env("QRTZ_MOCK_CMD_LOG");
    let insert_busy = Arc::new(Mutex::new(()));
    let crash_after: Option<u64> = env("QRTZ_MOCK_CRASH_AFTER_MS").and_then(|v| v.parse().ok());
    let crash_mark = env("QRTZ_MOCK_CRASH_MARK");
    let revoke: Vec<(String, u64)> = env("QRTZ_MOCK_REVOKE")
        .unwrap_or_default()
        .split(',')
        .filter_map(|kv| {
            let (p, ms) = kv.trim().split_once('@')?;
            matches!(p, "microphone" | "screen-recording" | "accessibility")
                .then_some(())
                .and(ms.parse().ok().map(|ms| (p.to_owned(), ms)))
        })
        .collect();
    let mut perms =
        json!({"accessibility": "granted", "microphone": "granted", "screen-recording": "granted"});
    for kv in env("QRTZ_MOCK_PERMISSIONS").unwrap_or_default().split(',') {
        if let Some((k, v)) = kv.split_once('=') {
            perms[k.trim()] = json!(v.trim());
        }
    }
    let perms = Arc::new(Mutex::new(perms));
    // The arbiter's view: denied permissions, whether recording / dictating.
    let denied: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(
        perms
            .lock()
            .unwrap()
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, v)| *v == "denied")
            .map(|(k, _)| k.clone())
            .collect(),
    ));
    let recording = Arc::new(AtomicBool::new(false));
    let stream_audio = AtomicBool::new(false);
    let paused = Arc::new(AtomicBool::new(false));
    let generation = Arc::new(AtomicU64::new(0));
    // The live dictation: (id, words heard, started).
    let dict: Arc<Mutex<Option<(String, usize, Instant)>>> = Arc::new(Mutex::new(None));
    let dict_gen = Arc::new(AtomicU64::new(0));
    let words: Arc<Vec<String>> = Arc::new(DICTATION.split(' ').map(String::from).collect());

    out.say(
        json!({"type": "hello", "protocol": 1, "helperVersion": "0.0.0-e2e", "os": "mock",
                   "features": if scenario { json!(["mock", "scenario:standup"]) } else { json!(["mock"]) }}),
    );
    let hb: u64 = env("QRTZ_MOCK_HB_MS").and_then(|v| v.parse().ok()).unwrap_or(200);
    if hb > 0 {
        let out = out.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(hb));
            out.say(json!({"type": "heartbeat", "ts": now()}));
        });
    }
    if scenario {
        let out = out.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            out.say(json!({"type": "ax.participants", "names": PARTICIPANTS}));
            out.say(json!({"type": "meeting.detected", "key": "zoom:fixture-standup", "title": "Weekly Product Sync",
                           "platform": "zoom", "app": "zoom.us"}));
        });
    }

    // Ends the dictation like the TS mock's stopDictation (final asr + dictation.stopped).
    let stop_dictation = {
        let (out, dict, dict_gen, words) = (out.clone(), dict.clone(), dict_gen.clone(), words.clone());
        move |id: &str, cancel: bool, reason: Option<&str>| {
            let mut d = dict.lock().unwrap();
            let Some((cur, heard, t0)) = d.clone() else { return };
            if cur != id {
                return;
            }
            dict_gen.fetch_add(1, Ordering::SeqCst);
            let text = if cancel {
                String::new()
            } else {
                words[..heard.max(1)].join(" ")
            };
            let dur = t0.elapsed().as_millis() as u64;
            if !cancel {
                out.say(json!({"type": "dictation.asr", "id": id, "event": {"kind": "final", "utteranceId": format!("{id}-1"),
                               "startMs": 0, "endMs": dur, "text": text}}));
            }
            let mut m = json!({"type": "dictation.stopped", "id": id, "text": text, "durationMs": if cancel { 0 } else { dur }});
            if let Some(r) = reason {
                m["reason"] = json!(r);
            }
            out.say(m);
            *d = None;
        }
    };
    let stop_dictation = Arc::new(stop_dictation);

    let revoke_armed = AtomicBool::new(false);
    let arm_revocations = || {
        if revoke_armed.swap(true, Ordering::SeqCst) {
            return;
        }
        for (p, ms) in revoke.clone() {
            let (out, denied, recording, perms, dict, stop) = (
                out.clone(),
                denied.clone(),
                recording.clone(),
                perms.clone(),
                dict.clone(),
                stop_dictation.clone(),
            );
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                let mut d = denied.lock().unwrap();
                if d.contains(&p) {
                    return;
                }
                let has = |x: &str| !d.contains(x) && x != p;
                let degraded: Vec<&str> = if !recording.load(Ordering::SeqCst) {
                    vec![]
                } else if p == "accessibility" {
                    vec!["ax-unavailable"]
                } else if p == "microphone" {
                    vec![if has("screen-recording") {
                        "no-mic"
                    } else {
                        "audio-unavailable"
                    }]
                } else {
                    vec![if has("microphone") {
                        "mic-only"
                    } else {
                        "audio-unavailable"
                    }]
                };
                d.insert(p.clone());
                drop(d);
                perms.lock().unwrap()[p.as_str()] = json!("denied");
                out.say(json!({"type": "permission.revoked", "permission": p, "atMs": now(), "degraded": degraded}));
                if p == "microphone" {
                    let live = dict.lock().unwrap().as_ref().map(|(id, _, _)| id.clone());
                    if let Some(id) = live {
                        stop(&id, false, Some("permission-revoked"));
                    }
                }
            });
        }
    };

    let mut audio: Option<String> = None;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(f) = &cmd_log {
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
                let _ = writeln!(
                    f,
                    "{} {}",
                    msg["type"].as_str().unwrap_or(""),
                    msg["id"].as_str().unwrap_or("")
                );
            }
        }
        match msg["type"].as_str().unwrap_or_default() {
            "summarize" | "summarize.check" | "summarize.cancel" => {
                if let Some(reply) = mock_summarize::handle(&line, &msg) {
                    out.say(reply);
                }
            }
            "ping" => out.say(json!({"type": "pong", "id": msg["id"]})),
            "permissions.check" | "permissions.request" => {
                out.say(json!({"type": "permissions.status", "status": *perms.lock().unwrap()}))
            }
            "shutdown" => std::process::exit(0),
            "recording.start" => {
                let path = msg["audioPath"].as_str().unwrap_or_default().to_owned();
                if recording.load(Ordering::SeqCst) {
                    out.say(json!({"type": "error", "code": "recording.busy", "message": "a recording is already running",
                                   "fatal": false, "audioPath": path, "activeAudioPath": audio}));
                    continue;
                }
                arm_revocations();
                recording.store(true, Ordering::SeqCst);
                paused.store(false, Ordering::SeqCst);
                let gen = generation.fetch_add(1, Ordering::SeqCst) + 1;
                let crash = match (&crash_after, &crash_mark) {
                    (Some(ms), Some(mark)) if !std::path::Path::new(mark).exists() => {
                        let _ = std::fs::write(mark, b"crashed");
                        Some(*ms)
                    }
                    _ => None,
                };
                if scenario {
                    match crash {
                        Some(ms) => {
                            let p = path.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(Duration::from_millis(ms));
                                let full = fixture_wav(&p, FIXTURE_DURATION_MS, false);
                                let half = 44 + (full.len() - 44) / 4 * 2;
                                let _ = std::fs::write(&p, &full[..half]);
                                std::process::exit(1);
                            });
                        }
                        None => {
                            let _ = std::fs::write(&path, fixture_wav(&path, FIXTURE_DURATION_MS, true));
                        }
                    }
                }
                let d = denied.lock().unwrap().clone();
                if d.contains("screen-recording") && d.contains("microphone") {
                    out.say(json!({"type": "error", "code": "audio.unavailable", "message": "no audio source could start",
                                   "fatal": false}));
                }
                out.say(json!({"type": "recording.started", "audioPath": path}));
                if scenario {
                    let (out, generation, paused) = (out.clone(), generation.clone(), paused.clone());
                    std::thread::spawn(move || {
                        let t0 = Instant::now();
                        for (t, m) in script() {
                            let at = Duration::from_millis((t as f64 * speed) as u64);
                            if let Some(wait) = at.checked_sub(t0.elapsed()) {
                                std::thread::sleep(wait);
                            }
                            if generation.load(Ordering::SeqCst) != gen {
                                return;
                            }
                            let p = paused.load(Ordering::SeqCst);
                            if m["type"] == "asr" && p {
                                continue;
                            }
                            if m["type"] == "audio.level" && p {
                                out.say(json!({"type": "audio.level", "rms": 0}));
                                continue;
                            }
                            out.say(m);
                        }
                    });
                }
                audio = Some(path);
            }
            "recording.stop" => {
                if let (Some(want), Some(cur)) = (msg["audioPath"].as_str(), audio.as_deref()) {
                    if want != cur {
                        continue;
                    }
                }
                if let Some(p) = audio.take() {
                    generation.fetch_add(1, Ordering::SeqCst);
                    recording.store(false, Ordering::SeqCst);
                    let dur = if scenario { FIXTURE_DURATION_MS } else { 0 };
                    out.say(json!({"type": "recording.stopped", "audioPath": p, "durationMs": dur}));
                }
            }
            "recording.abort" => {
                let p = msg["audioPath"].as_str().unwrap_or_default().to_owned();
                if audio.as_deref() == Some(p.as_str()) {
                    audio = None;
                    recording.store(false, Ordering::SeqCst);
                    generation.fetch_add(1, Ordering::SeqCst);
                }
                out.say(json!({"type": "recording.aborted", "audioPath": p}));
            }
            "recording.pause" => paused.store(true, Ordering::SeqCst),
            "recording.resume" => paused.store(false, Ordering::SeqCst),
            "speech.attach" => out.say(json!({"type": "speech.attach.rejected", "sessionId": msg["sessionId"],
                                              "reason": "unsupported"})),
            // `streamAudio`: the host runs a cloud engine and wants the mic as `dictation.audio` frames (TS mock dictCfg).
            "dictation.configure" => stream_audio.store(msg["streamAudio"] == true, Ordering::SeqCst),
            "dictation.start" => {
                let id = msg["id"].as_str().unwrap_or_default().to_owned();
                if let Some((cur, _, _)) = dict.lock().unwrap().as_ref() {
                    out.say(json!({"type": "error", "code": "dictation.busy", "message": "a dictation is already running",
                                   "fatal": false, "id": id, "activeId": cur}));
                    continue;
                }
                if denied.lock().unwrap().contains("microphone") {
                    out.say(json!({"type": "error", "code": "dictation.mic", "message": "microphone access denied",
                                   "fatal": false}));
                    continue;
                }
                arm_revocations();
                let gen = dict_gen.fetch_add(1, Ordering::SeqCst) + 1;
                *dict.lock().unwrap() = Some((id.clone(), 0, Instant::now()));
                let (out, dict, dict_gen, words) = (out.clone(), dict.clone(), dict_gen.clone(), words.clone());
                let stream = stream_audio.load(Ordering::SeqCst);
                std::thread::spawn(move || {
                    if start_ms > 0 {
                        std::thread::sleep(Duration::from_millis(start_ms));
                        if dict_gen.load(Ordering::SeqCst) != gen {
                            return;
                        }
                    }
                    out.say(json!({"type": "dictation.started", "id": id, "preRollMs": 0}));
                    for i in 1..=words.len() {
                        std::thread::sleep(Duration::from_millis(step_ms));
                        let mut d = dict.lock().unwrap();
                        if dict_gen.load(Ordering::SeqCst) != gen {
                            return;
                        }
                        if let Some(x) = d.as_mut() {
                            x.1 = i;
                        }
                        out.say(json!({"type": "dictation.asr", "id": id, "event": {"kind": "partial",
                                       "utteranceId": format!("{id}-1"), "startMs": 0, "text": words[..i].join(" ")}}));
                        out.say(json!({"type": "dictation.level", "rms": 0.3 + 0.5 * (i as f64).sin().abs()}));
                        if stream {
                            out.say(json!({"type": "dictation.audio", "id": id, "seq": i - 1, "pcm16": "AAAAAAAAAAA="}));
                        }
                    }
                });
            }
            "dictation.stop" => stop_dictation(msg["id"].as_str().unwrap_or_default(), false, None),
            "dictation.cancel" => stop_dictation(msg["id"].as_str().unwrap_or_default(), true, None),
            "insert.text" => {
                let r = match insert.as_str() {
                    "fail" => json!({"type": "insert.result", "id": msg["id"], "method": "none", "ok": false,
                                     "app": "Mock Editor", "error": "no focused text field"}),
                    "paste" => json!({"type": "insert.result", "id": msg["id"], "method": "paste", "ok": true, "app": "Mock Editor"}),
                    _ => json!({"type": "insert.result", "id": msg["id"], "method": "ax-selected-text", "ok": true,
                                "app": "Mock Editor"}),
                };
                if insert_delay_ms == 0 {
                    out.say(r);
                } else {
                    // One paste at a time, like the real helper: a later insert.text waits behind this one.
                    let (out, busy) = (out.clone(), insert_busy.clone());
                    std::thread::spawn(move || {
                        let _g = busy.lock().unwrap();
                        std::thread::sleep(Duration::from_millis(insert_delay_ms));
                        out.say(r);
                    });
                }
            }
            _ => out.say(json!({"type": "error", "code": "protocol.unknown", "message": "unknown message", "fatal": false})),
        }
    }
}
