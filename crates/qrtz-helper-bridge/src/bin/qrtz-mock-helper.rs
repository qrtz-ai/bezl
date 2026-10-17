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
//!   QRTZ_MOCK_SCENARIO=standup   play the scripted "Weekly Product Sync" (packages/core/src/note/fixtures.ts): hello
//!                                feature `scenario:standup`, `ax.participants` + `meeting.detected` 300 ms after start;
//!                                per recording the fixture's partial/final `asr`, `ax.activeSpeaker` and `audio.level`
//!                                lines, a fixture WAV at the audio path, `recording.stopped.durationMs` = 36500.
//!   QRTZ_MOCK_SPEED=0.05         time scale of the scenario script (0.05 = 20x real time; the Electron default).
//!   QRTZ_MOCK_CRASH_AFTER_MS=n   with a scenario: once per app run (per host pid), n ms after recording.start, leave a
//!                                WAV with an unfinalized header holding the first half of the fixture audio and exit(3).
//!   QRTZ_MOCK_REVOKE=microphone@1500,screen-recording@3000
//!                                H5: revoke a permission n ms after the first capture (recording.start or
//!                                dictation.start): `permission.revoked {permission, atMs, degraded}` (the arbiter's
//!                                degradations while recording) and a live dictation stops with `permission-revoked`.
//!   QRTZ_MOCK_INSERT=ax|paste|fail
//!                                `insert.result`: AX (default), paste, or nothing could insert (ok false, method none).
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

/// The standup fixture (packages/core/src/note/fixtures.ts STANDUP_FIXTURE): [speaker, startMs, endMs, text].
const STANDUP: &[(&str, u64, u64, &str)] = &[
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
const STANDUP_PARTICIPANTS: &[&str] = &["Ada Lovelace", "Grace Hopper", "Alan Turing"];
const FIXTURE_DURATION_MS: u64 = 36_500;

/// The scenario's per-recording script, fixture ms (unscaled): `fixtureAsrEvents` with the partials spread evenly
/// before each final, `fixtureActiveSpeakers` (250 ms lag) and an `audio.level` every 500 ms. `None` = a level line
/// (its rms depends on pause at send time).
fn standup_script() -> Vec<(f64, Option<Value>)> {
    let mut out: Vec<(f64, Option<Value>)> = Vec::new();
    for (i, (name, start, end, text)) in STANDUP.iter().enumerate() {
        let words: Vec<&str> = text.split(' ').collect();
        let id = format!("u{i}");
        let partials: Vec<usize> = (3..words.len()).step_by(4).collect();
        let count = partials.len() as f64 + 1.0;
        for (n, k) in partials.iter().enumerate() {
            let t = *start as f64 + (*end as f64 - *start as f64) * (n as f64 + 1.0) / (count + 0.5);
            out.push((
                t,
                Some(
                    json!({"type": "asr", "event": {"kind": "partial", "utteranceId": id, "startMs": start,
                    "text": words[..*k].join(" ")}}),
                ),
            ));
        }
        out.push((
            *end as f64,
            Some(json!({"type": "asr", "event": {"kind": "final", "utteranceId": id, "startMs": start, "endMs": end,
                "text": text, "confidence": 0.9}})),
        ));
        out.push((
            (*start + 250) as f64,
            Some(json!({"type": "ax.activeSpeaker", "atMs": start + 250, "name": name})),
        ));
    }
    for t in (0..FIXTURE_DURATION_MS).step_by(500) {
        out.push((t as f64, None));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// 16 kHz mono PCM16 with tone bursts where the fixture speakers talk (Electron `writeFixtureWav`). `finalized =
/// false` leaves the header sizes at 0, like a helper killed mid-recording.
fn fixture_wav(finalized: bool) -> Vec<u8> {
    let rate = 16_000u32;
    let n = (FIXTURE_DURATION_MS as f64 / 1000.0 * rate as f64).round() as usize;
    let mut b = Vec::with_capacity(44 + n * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n as u32 * 2).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(n as u32 * 2).to_le_bytes());
    let mut pcm = vec![0i16; n];
    let speakers = ["Ada Lovelace", "Grace Hopper", "Alan Turing"];
    for (who, start, end, _) in STANDUP {
        let f = 180.0 + 70.0 * speakers.iter().position(|s| s == who).unwrap_or(0) as f64;
        let (a, z) = (
            (*start as f64 / 1000.0 * rate as f64).round() as usize,
            (*end as f64 / 1000.0 * rate as f64).round() as usize,
        );
        for (i, v) in pcm.iter_mut().enumerate().take(z.min(n)).skip(a) {
            let env = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (rate as f64 * 0.3)).sin();
            *v = (3000.0 * env * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()).round()
                as i16;
        }
    }
    for v in pcm {
        b.extend_from_slice(&v.to_le_bytes());
    }
    if !finalized {
        b[4..8].copy_from_slice(&36u32.to_le_bytes());
        b[40..44].copy_from_slice(&0u32.to_le_bytes());
    }
    b
}

/// The arbiter's `degradationsForRevocation` (crates/qrtz-lifecycle arbiter, spec/vectors/helper/arbiter.json).
fn revocation_degraded(p: &str, denied: &[String]) -> &'static str {
    let has = |x: &str| x != p && !denied.iter().any(|d| d == x);
    match p {
        "accessibility" => "ax-unavailable",
        "microphone" if has("screen-recording") => "no-mic",
        "microphone" => "audio-unavailable",
        _ if has("microphone") => "mic-only",
        _ => "audio-unavailable",
    }
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
    let mut perms0 =
        json!({"accessibility": "granted", "microphone": "granted", "screen-recording": "granted"});
    for kv in env("QRTZ_MOCK_PERMISSIONS").unwrap_or_default().split(',') {
        if let Some((k, v)) = kv.split_once('=') {
            perms0[k.trim()] = json!(v.trim());
        }
    }
    let denied: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(
        perms0
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, v)| *v == "denied")
            .map(|(k, _)| k.clone())
            .collect(),
    ));
    let perms = Arc::new(Mutex::new(perms0));
    let scenario = env("QRTZ_MOCK_SCENARIO").filter(|v| v == "standup").is_some();
    let speed = env("QRTZ_MOCK_SPEED")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(0.05);
    let crash_after = env("QRTZ_MOCK_CRASH_AFTER_MS")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0);
    let insert_mode = env("QRTZ_MOCK_INSERT").filter(|v| v == "ax" || v == "paste" || v == "fail");
    let revoke: Vec<(String, u64)> = env("QRTZ_MOCK_REVOKE")
        .unwrap_or_default()
        .split(',')
        .filter_map(|kv| {
            let (p, ms) = kv.trim().split_once('@')?;
            ["microphone", "screen-recording", "accessibility"]
                .contains(&p)
                .then_some(())?;
            Some((p.to_owned(), ms.parse().ok()?))
        })
        .collect();
    let mut revoke_armed = false;
    let recording = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut features = vec!["mock".to_string()];
    if scenario {
        features.push("scenario:standup".into());
    }
    say(
        json!({"type": "hello", "protocol": 1, "helperVersion": "0.0.0-mock", "os": "mock", "features": features}),
    );
    if scenario {
        let say = say.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            say(json!({"type": "ax.participants", "names": STANDUP_PARTICIPANTS}));
            say(
                json!({"type": "meeting.detected", "key": "zoom:fixture-standup", "title": "Weekly Product Sync",
                "platform": "zoom", "app": "zoom.us"}),
            );
        });
    }
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

    let dictation_id: Arc<Mutex<Option<String>>> = Arc::default();

    let live_targets: Arc<Mutex<std::collections::HashSet<String>>> = Default::default();
    let mut started = 0u64;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match msg["type"].as_str().unwrap_or_default() {
            "ping" => say(json!({"type": "pong", "id": msg["id"]})),
            "permissions.check" | "permissions.request" => {
                say(json!({"type": "permissions.status", "status": perms.lock().unwrap().clone()}))
            }
            "recording.start" => {
                let path = msg["audioPath"].as_str().unwrap_or_default().to_owned();
                started = now();
                let gen = generation.fetch_add(1, Ordering::SeqCst) + 1;
                recording.store(true, Ordering::SeqCst);
                paused.store(false, Ordering::SeqCst);
                if !revoke_armed {
                    revoke_armed = true;
                    arm_revocations(
                        &revoke,
                        &say,
                        &denied,
                        &perms,
                        &recording,
                        &dictation_id,
                        &dictation_gen,
                        &dictation_heard,
                        &dictation_words,
                        &live_targets,
                    );
                }
                if scenario && !path.is_empty() {
                    // Like Electron's host-side writeFixtureWav: the fixture audio is on disk from the start.
                    let crash_marker = std::env::temp_dir().join(format!(
                        "qrtz-mock-crashed-{}",
                        std::os::unix::process::parent_id()
                    ));
                    match crash_after.filter(|_| !crash_marker.exists()) {
                        Some(ms) => {
                            let _ = std::fs::write(&crash_marker, b"crashed");
                            let p = path.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(Duration::from_millis(ms));
                                let w = fixture_wav(false);
                                let half = 44 + (w.len() - 44) / 4 * 2;
                                let _ = std::fs::write(&p, &w[..half]);
                                std::process::exit(3);
                            });
                        }
                        None => {
                            let _ = std::fs::write(&path, fixture_wav(true));
                        }
                    }
                }
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
                if scenario {
                    let (say, generation, paused) = (say.clone(), generation.clone(), paused.clone());
                    std::thread::spawn(move || {
                        let t0 = std::time::Instant::now();
                        for (t, line) in standup_script() {
                            let at = Duration::from_secs_f64(t * speed / 1000.0);
                            if let Some(d) = at.checked_sub(t0.elapsed()) {
                                std::thread::sleep(d);
                            }
                            if generation.load(Ordering::SeqCst) != gen {
                                return;
                            }
                            let p = paused.load(Ordering::SeqCst);
                            match line {
                                None => say(json!({"type": "audio.level",
                                    "rms": if p { 0.0 } else { 0.2 + 0.6 * (t / 700.0).sin().abs() }})),
                                Some(l) if l["type"] == "asr" && p => {}
                                Some(l) => say(l),
                            }
                        }
                    });
                } else {
                    say(
                        json!({"type": "asr", "event": {"kind": "partial", "utteranceId": "u1", "text": "hello", "startMs": 0}}),
                    );
                    say(
                        json!({"type": "asr", "event": {"kind": "final", "utteranceId": "u1", "text": "Hello from the mock helper.", "startMs": 0, "endMs": 1200}}),
                    );
                }
                live_targets.lock().unwrap().insert(format!("r:{path}"));
                audio = Some(path);
            }
            "recording.stop" => {
                generation.fetch_add(1, Ordering::SeqCst);
                recording.store(false, Ordering::SeqCst);
                if let Some(p) = &audio {
                    live_targets.lock().unwrap().remove(&format!("r:{p}"));
                }
                if let Some(p) = audio.take() {
                    let d = if scenario {
                        FIXTURE_DURATION_MS
                    } else {
                        now() - started
                    };
                    say(json!({"type": "recording.stopped", "audioPath": p, "durationMs": d}));
                }
            }
            "recording.abort" => {
                // N2: scoped teardown without a flush; answered even when nothing records that file.
                let p = msg["audioPath"].as_str().unwrap_or_default().to_owned();
                if audio.as_deref() == Some(p.as_str()) {
                    audio = None;
                    generation.fetch_add(1, Ordering::SeqCst);
                    recording.store(false, Ordering::SeqCst);
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
                if !revoke_armed {
                    revoke_armed = true;
                    arm_revocations(
                        &revoke,
                        &say,
                        &denied,
                        &perms,
                        &recording,
                        &dictation_id,
                        &dictation_gen,
                        &dictation_heard,
                        &dictation_words,
                        &live_targets,
                    );
                }
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
                *dictation_id.lock().unwrap() = Some(msg["id"].as_str().unwrap_or_default().to_owned());
                live_targets
                    .lock()
                    .unwrap()
                    .insert(format!("d:{}", msg["id"].as_str().unwrap_or_default()));
            }
            ty @ ("dictation.stop" | "dictation.cancel") => {
                let id = msg["id"].as_str().unwrap_or_default().to_owned();
                let mut cur = dictation_id.lock().unwrap();
                if cur.as_deref() == Some(id.as_str()) {
                    dictation_gen.fetch_add(1, Ordering::SeqCst);
                    *cur = None;
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
            "insert.text" => say(match insert_mode.as_deref() {
                Some("fail") => {
                    json!({"type": "insert.result", "id": msg["id"], "method": "none", "ok": false,
                    "app": "Mock Editor", "error": "no focused text field"})
                }
                Some("paste") => {
                    json!({"type": "insert.result", "id": msg["id"], "method": "paste", "ok": true,
                    "app": "Mock Editor"})
                }
                _ => {
                    json!({"type": "insert.result", "id": msg["id"], "method": "ax-selected-text", "ok": true,
                    "app": "Mock Editor"})
                }
            }),
            "recording.pause" => paused.store(true, Ordering::SeqCst),
            "recording.resume" => paused.store(false, Ordering::SeqCst),
            "shutdown" => std::process::exit(0),
            _ => say(
                json!({"type": "error", "code": "protocol.unknown", "message": "unknown message", "fatal": false}),
            ),
        }
    }
}

/// H5: each scripted revocation fires once, `ms` after the first capture. Like the Swift helper (and the arbiter):
/// a permission already denied is unchanged; the mic going away ends a live dictation (`permission-revoked`);
/// while recording, `degraded` names what the recording lost.
#[allow(clippy::too_many_arguments)]
fn arm_revocations(
    revoke: &[(String, u64)],
    say: &(impl Fn(Value) + Clone + Send + Sync + 'static),
    denied: &Arc<Mutex<Vec<String>>>,
    perms: &Arc<Mutex<Value>>,
    recording: &Arc<std::sync::atomic::AtomicBool>,
    dictation_id: &Arc<Mutex<Option<String>>>,
    dictation_gen: &Arc<AtomicU64>,
    dictation_heard: &Arc<AtomicU64>,
    words: &Arc<Vec<String>>,
    live: &Arc<Mutex<std::collections::HashSet<String>>>,
) {
    for (p, ms) in revoke.iter().cloned() {
        let (say, denied, perms, recording) = (say.clone(), denied.clone(), perms.clone(), recording.clone());
        let (dictation_id, dictation_gen, heard, words, live) = (
            dictation_id.clone(),
            dictation_gen.clone(),
            dictation_heard.clone(),
            words.clone(),
            live.clone(),
        );
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            let degraded = {
                let mut d = denied.lock().unwrap();
                if d.contains(&p) {
                    return;
                }
                let deg = recording
                    .load(Ordering::SeqCst)
                    .then(|| revocation_degraded(&p, &d));
                d.push(p.clone());
                deg
            };
            perms.lock().unwrap()[p.as_str()] = json!("denied");
            say(
                json!({"type": "permission.revoked", "permission": p, "atMs": now(),
                "degraded": degraded.into_iter().collect::<Vec<_>>()}),
            );
            if p == "microphone" {
                if let Some(id) = dictation_id.lock().unwrap().take() {
                    dictation_gen.fetch_add(1, Ordering::SeqCst);
                    live.lock().unwrap().remove(&format!("d:{id}"));
                    let n = heard.load(Ordering::SeqCst).clamp(1, words.len() as u64) as usize;
                    say(
                        json!({"type": "dictation.stopped", "id": id, "text": words[..n].join(" "),
                        "durationMs": 1000, "reason": "permission-revoked"}),
                    );
                }
            }
        });
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
