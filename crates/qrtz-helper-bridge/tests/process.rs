//! Real child processes: the Rust mock helper (always) and the real Swift helper (when QRTZ_HELPER_BIN or a
//! Linux SwiftPM build exists). Also the golden protocol fixtures shared with Swift and TS.
use qrtz_helper_bridge::*;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn fixture(name: &str) -> Vec<String> {
    std::fs::read_to_string(root().join("native/qrtz-helper/Fixtures/protocol").join(name))
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

#[test]
fn golden_helper_to_host_fixtures_parse_and_reencode_byte_for_byte() {
    for l in fixture("helper-to-host.jsonl") {
        let v = parse_helper_line(&l).unwrap_or_else(|e| panic!("{l}: {e}"));
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            l,
            "sorted-key re-encode differs"
        );
    }
}

#[test]
fn golden_host_to_helper_fixtures_encode_byte_for_byte() {
    for l in fixture("host-to-helper.jsonl") {
        let v: Value = serde_json::from_str(&l).unwrap();
        assert_eq!(encode_host_message(&v).unwrap(), format!("{l}\n"));
    }
}

#[test]
fn golden_invalid_fixtures_are_rejected() {
    for l in fixture("invalid-helper-to-host.jsonl") {
        assert!(parse_helper_line(&l).is_err(), "{l}");
    }
    assert_eq!(parse_helper_line("   ").unwrap_err(), "empty");
    assert_eq!(parse_helper_line("{nope").unwrap_err(), "invalid json");
    assert!(
        encode_host_message(&json!({"type": "recording.start"})).is_err(),
        "audioPath required"
    );
}

#[test]
fn fixtures_cover_every_message_type() {
    let p = protocol();
    let types = |f: &str| -> std::collections::BTreeSet<String> {
        fixture(f)
            .iter()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    };
    assert_eq!(
        types("helper-to-host.jsonl"),
        p.helper_to_host_types.iter().cloned().collect()
    );
    assert_eq!(
        types("host-to-helper.jsonl"),
        p.host_to_helper_types.iter().cloned().collect()
    );
}

fn mock(env: &[(&str, &str)]) -> ProcessSpawn {
    let mut s = ProcessSpawn::helper(PathBuf::from(env!("CARGO_BIN_EXE_qrtz-mock-helper")));
    s.env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    s
}
fn fast() -> SupervisorOptions {
    SupervisorOptions {
        base_delay: Duration::from_millis(50),
        max_delay: Duration::from_millis(200),
        heartbeat_timeout: Duration::from_millis(1500),
        ..Default::default()
    }
}
async fn wait_state(s: &Supervisor, want: HelperState) {
    let mut w = s.watch_status();
    timeout(Duration::from_secs(10), async {
        while w.borrow_and_update().state != want {
            w.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never reached {want:?}; at {:?}", s.status()));
}
async fn next_of(rx: &mut tokio::sync::broadcast::Receiver<String>, ty: &str) -> Value {
    timeout(Duration::from_secs(10), async {
        loop {
            let l = rx.recv().await.unwrap();
            if let Ok(v) = parse_helper_line(&l) {
                if v["type"] == ty {
                    return v;
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {ty}"))
}

#[tokio::test]
async fn real_process_hello_ping_permissions_recording_shutdown() {
    let s = Supervisor::spawn(
        fast(),
        Arc::new(mock(&[("QRTZ_MOCK_PERMISSIONS", "microphone=denied")])),
    );
    let mut rx = s.subscribe_lines();
    s.start();
    assert_eq!(next_of(&mut rx, "hello").await["protocol"], 1);
    wait_state(&s, HelperState::Running).await;
    let send = |v: Value| encode_host_message(&v).unwrap();
    assert!(s.send(send(json!({"type": "ping", "id": "p1"}))).await);
    assert_eq!(next_of(&mut rx, "pong").await["id"], "p1");
    assert!(s.send(send(json!({"type": "permissions.check"}))).await);
    assert_eq!(
        next_of(&mut rx, "permissions.status").await["status"]["microphone"],
        "denied"
    );
    assert!(
        s.send(send(
            json!({"type": "recording.start", "audioPath": "/tmp/x.wav"})
        ))
        .await
    );
    assert_eq!(
        next_of(&mut rx, "recording.started").await["audioPath"],
        "/tmp/x.wav"
    );
    assert_eq!(next_of(&mut rx, "asr").await["event"]["kind"], "partial");
    assert!(s.send(send(json!({"type": "recording.stop"}))).await);
    next_of(&mut rx, "recording.stopped").await;
    next_of(&mut rx, "heartbeat").await;
    s.shutdown(send(json!({"type": "shutdown"})), Duration::from_secs(2))
        .await;
    assert_eq!(s.status().state, HelperState::Stopped);
}

#[tokio::test]
async fn real_process_scripted_capture_fallback_silent_audible() {
    // N29 + H8 knobs of the Rust mock helper, for NoteRuntime integration runs.
    let s = Supervisor::spawn(
        fast(),
        Arc::new(mock(&[
            ("QRTZ_MOCK_FALLBACK", "screen>mic,mic>"),
            ("QRTZ_MOCK_SILENT", "mic@50-500,system@250,junk@1"),
        ])),
    );
    let mut rx = s.subscribe_lines();
    s.start();
    wait_state(&s, HelperState::Running).await;
    let send = |v: Value| encode_host_message(&v).unwrap();
    assert!(
        s.send(send(
            json!({"type": "recording.start", "audioPath": "/tmp/x.wav"})
        ))
        .await
    );
    // Wire order matters: fallbacks, then audio.unavailable, then recording.started.
    let mut seen = vec![];
    while seen.last().map(|v: &Value| v["type"].clone()) != Some(json!("recording.started")) {
        let v = parse_helper_line(
            &timeout(Duration::from_secs(10), rx.recv())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        if v["type"] != "heartbeat" && v["type"] != "hello" {
            seen.push(v);
        }
    }
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert_eq!(
        seen[0],
        json!({"type": "capture.fallback", "from": "screen", "to": "mic", "reason": "mock: source failed to start"})
    );
    assert_eq!(seen[1]["to"], Value::Null);
    assert_eq!(seen[2]["code"], "audio.unavailable");
    let silent = next_of(&mut rx, "capture.silent").await;
    assert_eq!(
        (silent["source"].as_str(), silent["sinceMs"].as_u64()),
        (Some("mic"), Some(0))
    );
    assert_eq!(next_of(&mut rx, "capture.silent").await["source"], "system");
    assert_eq!(next_of(&mut rx, "capture.audible").await["source"], "mic");
    // A stop cancels the script of that recording: nothing scripted follows.
    assert!(
        s.send(send(
            json!({"type": "recording.start", "audioPath": "/tmp/y.wav"})
        ))
        .await
    );
    next_of(&mut rx, "recording.started").await;
    assert!(s.send(send(json!({"type": "recording.stop"}))).await);
    next_of(&mut rx, "recording.stopped").await;
    let late = timeout(Duration::from_millis(400), next_of(&mut rx, "capture.silent")).await;
    assert!(late.is_err(), "scripted silence after stop: {late:?}");
    s.stop();
}

#[tokio::test]
async fn real_process_mic_fails_sends_capture_degraded_and_drops_mic_silence() {
    // N35 knob of the Rust mock helper (QRTZ_MOCK_MIC_FAILS, same as the TS mock's micFails).
    let s = Supervisor::spawn(
        fast(),
        Arc::new(mock(&[
            ("QRTZ_MOCK_MIC_FAILS", "50"),
            ("QRTZ_MOCK_SILENT", "mic@300,system@400"),
        ])),
    );
    let mut rx = s.subscribe_lines();
    s.start();
    wait_state(&s, HelperState::Running).await;
    let send = |v: Value| encode_host_message(&v).unwrap();
    assert!(
        s.send(send(
            json!({"type": "recording.start", "audioPath": "/tmp/m.wav"})
        ))
        .await
    );
    let d = next_of(&mut rx, "capture.degraded").await;
    assert_eq!(
        d,
        json!({"type": "capture.degraded", "degraded": ["no-mic"], "reason": "mock: mic device failed"})
    );
    // The mic is not tracked any more: the first silence after the failure is the system's.
    assert_eq!(next_of(&mut rx, "capture.silent").await["source"], "system");
    s.stop();
}

#[tokio::test]
async fn real_process_crash_restarts_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("crashed");
    let s = Supervisor::spawn(
        fast(),
        Arc::new(mock(&[("QRTZ_MOCK_CRASH_ONCE", marker.to_str().unwrap())])),
    );
    let mut rx = s.subscribe_lines();
    s.start();
    next_of(&mut rx, "hello").await;
    wait_state(&s, HelperState::Backoff).await;
    assert!(s.status().last_error.unwrap().contains("code=3"));
    next_of(&mut rx, "hello").await;
    wait_state(&s, HelperState::Running).await;
    assert_eq!(s.status().restarts, 1);
    s.stop();
}

#[tokio::test]
async fn real_process_silent_helper_hits_heartbeat_timeout() {
    let mut o = fast();
    o.heartbeat_timeout = Duration::from_millis(300);
    let s = Supervisor::spawn(o, Arc::new(mock(&[("QRTZ_MOCK_SILENT", "1")])));
    s.start();
    wait_state(&s, HelperState::Backoff).await;
    assert_eq!(s.status().last_error.as_deref(), Some("heartbeat timeout"));
    s.stop();
}

#[tokio::test]
async fn real_process_crash_loop_opens_the_circuit() {
    let mut o = fast();
    o.max_restarts = 2;
    let s = Supervisor::spawn(o, Arc::new(mock(&[("QRTZ_MOCK_EXIT_AFTER_MS", "20")])));
    s.start();
    wait_state(&s, HelperState::Failed).await;
    assert_eq!(s.status().restarts, 2);
}

#[tokio::test]
async fn missing_binary_backs_off_instead_of_panicking() {
    let s = Supervisor::spawn(
        fast(),
        Arc::new(ProcessSpawn::helper("/nonexistent/qrtz-helper".into())),
    );
    s.start();
    wait_state(&s, HelperState::Backoff).await;
    s.stop();
}

#[tokio::test]
async fn invalid_lines_are_forwarded_verbatim_but_fail_validation() {
    let s = Supervisor::spawn(fast(), Arc::new(mock(&[("QRTZ_MOCK_GARBAGE", "1")])));
    let mut rx = s.subscribe_lines();
    s.start();
    let mut saw_garbage = false;
    for _ in 0..3 {
        let l = timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        if l == "{not json" {
            saw_garbage = parse_helper_line(&l).is_err();
            break;
        }
    }
    assert!(saw_garbage);
    s.stop();
}

/// The real Swift helper (Linux stub build or a macOS build), unchanged protocol end to end.
#[tokio::test]
async fn real_swift_helper_when_available() {
    let candidates = [
        std::env::var("QRTZ_HELPER_BIN").ok().map(PathBuf::from),
        Some(root().join("native/qrtz-helper/.build/x86_64-unknown-linux-gnu/debug/qrtz-helper")),
        Some(root().join("native/qrtz-helper/.build/release/qrtz-helper")),
    ];
    let Some(bin) = candidates.into_iter().flatten().find(|p| p.exists()) else {
        eprintln!("SKIP: no Swift helper build (set QRTZ_HELPER_BIN or run bun run test:helper)");
        return;
    };
    let s = Supervisor::spawn(fast(), Arc::new(ProcessSpawn::helper(bin.clone())));
    let mut rx = s.subscribe_lines();
    s.start();
    let hello = next_of(&mut rx, "hello").await;
    assert_eq!(hello["protocol"], protocol().version, "{bin:?}");
    assert!(
        s.send(encode_host_message(&json!({"type": "ping", "id": "sw"})).unwrap())
            .await
    );
    assert_eq!(next_of(&mut rx, "pong").await["id"], "sw");
    s.shutdown(
        encode_host_message(&json!({"type": "shutdown"})).unwrap(),
        Duration::from_secs(3),
    )
    .await;
}

/// N38: `QRTZ_MOCK_MIC_FAILS=start` writes capture.degraded in the same stdout chunk as recording.started (the Swift
/// `goLive` ordering that hid N37), so host tests exercise the ack-hold with both lines in one read.
#[test]
fn mock_mic_fails_at_start_shares_the_started_chunk() {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_qrtz-mock-helper"))
        .env("QRTZ_MOCK_HB_MS", "0")
        .env("QRTZ_MOCK_MIC_FAILS", "start")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut hello = String::new();
    out.read_line(&mut hello).unwrap();
    assert!(hello.contains("\"hello\""));
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"type":"recording.start","audioPath":"/tmp/n38.wav","sources":["mic"]}}"#
    )
    .unwrap();
    // One read returns everything written so far by the one write.
    let mut buf = [0u8; 4096];
    let n = out.get_mut().read(&mut buf).unwrap();
    let chunk = std::str::from_utf8(&buf[..n]).unwrap();
    let lines: Vec<Value> = chunk.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert!(lines.len() >= 2, "{chunk}");
    assert_eq!(lines[0]["type"], "recording.started");
    assert_eq!(
        lines[1],
        serde_json::json!({"type": "capture.degraded", "degraded": ["no-mic"], "reason": "mock: mic device failed"})
    );
    for l in &lines {
        qrtz_helper_bridge::parse_helper_line(&l.to_string()).unwrap();
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
}
