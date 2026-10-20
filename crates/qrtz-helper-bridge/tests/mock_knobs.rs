#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! The Rust mock helper honours every knob `apps/desktop/src-tauri/src/paths.rs` passes (parity with the Electron TS
//! mock): QRTZ_MOCK_SCENARIO, SPEED, REVOKE, INSERT and CRASH_AFTER_MS. Every line it prints must pass the
//! helper → host schema.
use qrtz_helper_bridge::parse_helper_line;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Mock {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Value>,
}

fn spawn(env: &[(&str, &str)]) -> Mock {
    let mut c = Command::new(env!("CARGO_BIN_EXE_qrtz-mock-helper"));
    c.env("QRTZ_MOCK_HB_MS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let out = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines() {
            let Ok(l) = l else { return };
            let v = parse_helper_line(&l).unwrap_or_else(|e| panic!("invalid helper line {l}: {e}"));
            if tx.send(v).is_err() {
                return;
            }
        }
    });
    Mock { child, stdin, rx }
}

impl Mock {
    fn send(&mut self, v: Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }
    fn until(&self, ty: &str, ms: u64) -> Value {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(v) if v["type"] == ty => return v,
                Ok(_) => {}
                Err(_) => panic!("no {ty} within {ms} ms"),
            }
        }
    }
    fn collect(&self, ms: u64) -> Vec<Value> {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut out = Vec::new();
        while let Ok(v) = self
            .rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
        {
            out.push(v);
        }
        out
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_standup_scenario_plays_the_fixture_meeting_and_writes_its_audio() {
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("r.wav");
    let path = wav.to_str().unwrap();
    let mut m = spawn(&[("QRTZ_MOCK_SCENARIO", "standup"), ("QRTZ_MOCK_SPEED", "0.01")]);
    let hello = m.until("hello", 2000);
    assert!(
        hello["features"]
            .as_array()
            .unwrap()
            .contains(&json!("scenario:standup")),
        "{hello}"
    );
    assert_eq!(
        m.until("ax.participants", 2000)["names"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let d = m.until("meeting.detected", 2000);
    assert_eq!(d["title"], "Weekly Product Sync");
    m.send(json!({"type": "recording.start", "audioPath": path}));
    m.until("recording.started", 2000);
    // 36.5 s of meeting at speed 0.01: about 0.4 s.
    let lines = m.collect(1500);
    let finals: Vec<&Value> = lines
        .iter()
        .filter(|v| v["type"] == "asr" && v["event"]["kind"] == "final")
        .collect();
    assert_eq!(finals.len(), 8, "{lines:?}");
    assert_eq!(
        finals[7]["event"]["text"],
        "Perfect, let's ship the desktop launch next week. Thanks all."
    );
    assert!(lines
        .iter()
        .any(|v| v["type"] == "asr" && v["event"]["kind"] == "partial"));
    assert!(lines
        .iter()
        .any(|v| v["type"] == "ax.activeSpeaker" && v["name"] == "Grace Hopper"));
    assert!(lines.iter().filter(|v| v["type"] == "audio.level").count() >= 70);
    assert_eq!(std::fs::metadata(&wav).unwrap().len(), 44 + 584_000 * 2);
    m.send(json!({"type": "recording.stop", "audioPath": path}));
    assert_eq!(m.until("recording.stopped", 2000)["durationMs"], 36_500);
}

#[test]
fn insertion_outcomes_follow_qrtz_mock_insert() {
    for (mode, method, ok) in [
        ("ax", "ax-selected-text", true),
        ("paste", "paste", true),
        ("fail", "none", false),
    ] {
        let mut m = spawn(&[("QRTZ_MOCK_INSERT", mode)]);
        m.until("hello", 2000);
        m.send(json!({"type": "insert.text", "id": "i1", "text": "hi"}));
        let r = m.until("insert.result", 2000);
        assert_eq!(
            (r["method"].as_str().unwrap(), r["ok"].as_bool().unwrap()),
            (method, ok),
            "{mode}: {r}"
        );
    }
}

#[test]
fn a_scripted_revocation_degrades_the_recording_and_ends_a_dictation() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("r.wav");
    let mut m = spawn(&[("QRTZ_MOCK_REVOKE", "microphone@200,screen-recording@400")]);
    m.until("hello", 2000);
    m.send(json!({"type": "recording.start", "audioPath": path.to_str().unwrap()}));
    m.until("recording.started", 2000);
    m.send(json!({"type": "dictation.start", "id": "d1"}));
    m.until("dictation.started", 2000);
    let r = m.until("permission.revoked", 2000);
    assert_eq!(
        (r["permission"].clone(), r["degraded"].clone()),
        (json!("microphone"), json!(["no-mic"]))
    );
    let s = m.until("dictation.stopped", 2000);
    assert_eq!(s["reason"], "permission-revoked", "{s}");
    // The second one: no mic left, so the recording has no audio at all.
    let r = m.until("permission.revoked", 2000);
    assert_eq!(r["degraded"], json!(["audio-unavailable"]), "{r}");
    m.send(json!({"type": "permissions.check"}));
    let p = m.until("permissions.status", 2000);
    assert_eq!(p["status"]["microphone"], "denied");
    assert_eq!(p["status"]["screen-recording"], "denied");
}

#[test]
fn crash_after_ms_kills_the_helper_once_per_host_with_a_half_written_wav() {
    let marker = std::env::temp_dir().join(format!("qrtz-mock-crashed-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("r.wav");
    let env = [
        ("QRTZ_MOCK_SCENARIO", "standup"),
        ("QRTZ_MOCK_SPEED", "1"),
        ("QRTZ_MOCK_CRASH_AFTER_MS", "200"),
    ];
    let mut m = spawn(&env);
    m.until("hello", 2000);
    m.send(json!({"type": "recording.start", "audioPath": wav.to_str().unwrap()}));
    let t0 = Instant::now();
    let st = loop {
        if let Some(st) = m.child.try_wait().unwrap() {
            break st;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "the mock did not crash");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(st.code(), Some(3));
    let b = std::fs::read(&wav).unwrap();
    assert_eq!(b.len(), 44 + 584_000);
    assert_eq!(&b[40..44], &0u32.to_le_bytes(), "unfinalized header");
    // The restarted helper (same host) records normally.
    let mut m = spawn(&env);
    m.until("hello", 2000);
    m.send(json!({"type": "recording.start", "audioPath": wav.to_str().unwrap()}));
    m.until("recording.started", 2000);
    std::thread::sleep(Duration::from_millis(400));
    assert!(m.child.try_wait().unwrap().is_none(), "crashed twice");
    assert_eq!(std::fs::metadata(&wav).unwrap().len(), 44 + 584_000 * 2);
    let _ = std::fs::remove_file(&marker);
}
