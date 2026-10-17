//! A stand-in for the Swift qrtz-helper that speaks the same protocol, for Rust integration tests and Linux
//! dev runs of the Tauri build. Knobs (env):
//!   QRTZ_MOCK_HB_MS=200          heartbeat interval (0 = no heartbeats)
//!   QRTZ_MOCK_SILENT=1           never print anything (heartbeat-timeout path)
//!   QRTZ_MOCK_EXIT_AFTER_MS=n    exit(3) after n ms
//!   QRTZ_MOCK_CRASH_ONCE=path    crash (exit 3) 100 ms after start unless `path` exists; creates it first
//!   QRTZ_MOCK_GARBAGE=1          print one invalid line right after hello
//!   QRTZ_MOCK_PERMISSIONS=microphone=denied,...
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn main() {
    let silent = env("QRTZ_MOCK_SILENT").as_deref() == Some("1");
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
                say(json!({"type": "recording.started", "audioPath": path}));
                say(
                    json!({"type": "asr", "event": {"kind": "partial", "utteranceId": "u1", "text": "hello", "startMs": 0}}),
                );
                say(
                    json!({"type": "asr", "event": {"kind": "final", "utteranceId": "u1", "text": "Hello from the mock helper.", "startMs": 0, "endMs": 1200}}),
                );
                audio = Some(path);
            }
            "recording.stop" => {
                if let Some(p) = audio.take() {
                    say(json!({"type": "recording.stopped", "audioPath": p, "durationMs": now() - started}));
                }
            }
            "recording.abort" => {
                // N2: scoped teardown without a flush; answered even when nothing records that file.
                let p = msg["audioPath"].as_str().unwrap_or_default().to_owned();
                if audio.as_deref() == Some(p.as_str()) {
                    audio = None;
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
