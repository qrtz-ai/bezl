//! N83: the Tauri backend runs the full step chain, LlmSteps (the configured provider) → CoreSteps (finalize;
//! index is a no-op since search is full-text), end to end through `build_app` and the router with the Rust mock helper. The provider is a local
//! mock OpenRouter (never the real network) reached through the `QRTZ_LLM_BASE_URL` dev knob.
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Shell;
impl ShellHooks for Shell {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
    fn notify(&self, _n: Notice) {}
}

/// One scripted HTTP/1.1 reply per connection; every request is recorded.
#[derive(Clone, Default)]
struct MockLlm {
    seen: Arc<Mutex<Vec<(String, String, String)>>>, // (request line, lower-cased head, body)
    replies: Arc<Mutex<VecDeque<String>>>,
}

impl MockLlm {
    async fn start(&self) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let me = self.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let me = me.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 8192];
                    let (head_end, len) = loop {
                        let n = s.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            break (i + 4, len);
                        }
                    };
                    while buf.len() < head_end + len {
                        let n = s.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let first = head.lines().next().unwrap_or("").to_string();
                    let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                    me.seen.lock().unwrap().push((first, head.to_lowercase(), body));
                    let reply = me.replies.lock().unwrap().pop_front().unwrap_or_else(|| {
                        http(
                            500,
                            "application/json",
                            r#"{"error":{"message":"no reply scripted"}}"#,
                        )
                    });
                    let _ = s.write_all(reply.as_bytes()).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        base
    }
    fn seen(&self) -> Vec<(String, String, String)> {
        self.seen.lock().unwrap().clone()
    }
}

fn http(status: u16, ct: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\ncontent-type: {ct}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn mock_helper_bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let p = exe.parent().unwrap().parent().unwrap().join("qrtz-mock-helper");
    if !p.exists() {
        let st = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "-q",
                "-p",
                "qrtz-helper-bridge",
                "--bin",
                "qrtz-mock-helper",
            ])
            .status()
            .unwrap();
        assert!(st.success());
    }
    p
}

fn app(dir: &Path, llm_base: &str) -> BuiltApp {
    build_app(AppBuild {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        data_dir: dir.to_path_buf(),
        helper: Some((
            ProcessSpawn::helper(mock_helper_bin()),
            HelperAvailability::Simulated,
        )),
        supervisor: SupervisorOptions::default(),
        shell: Arc::new(Shell),
        storage_shell: Arc::new(NoStorageShell),
        secrets: None,
        talk: None,
        speech: None,
        steps: None,
        knobs: DevKnobs {
            retry_scale: Some(0.001),
            llm_base_url: Some(llm_base.into()),
            ..Default::default()
        },
    })
    .unwrap()
}

async fn call(r: &Router<Shared>, sub: &str, ch: &str, payload: Value) -> Value {
    let v = r
        .dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":sub,"payload":payload}))
        .await;
    assert_eq!(v["ok"], true, "{ch}: {v}");
    v["data"].clone()
}

async fn until<F: Fn() -> bool>(what: &str, f: F) {
    for _ in 0..600 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Record with the mock helper (it finalizes one line), stop, wait for the pipeline to settle.
async fn record_one(a: &BuiltApp, r: &Router<Shared>, title: &str) -> (String, Value) {
    let svc = a.service.clone();
    until("helper", || svc.helper_ready()).await;
    let s = call(r, "note", "note:start", json!({"title": title})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    call(r, "note", "note:stop", json!(null)).await;
    let rt2 = a.runtime.clone();
    until("done", || rt2.session().phase.as_str() == "done").await;
    // `done` is the session; wait for the pipeline's last step too.
    let mut p = Value::Null;
    for _ in 0..400 {
        p = call(r, "note", "note:pipeline", json!({"id": id})).await;
        if ["summary", "index"].iter().all(|s| {
            matches!(
                p["steps"][s]["status"].as_str(),
                Some("done" | "failed" | "skipped")
            )
        }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    (id, p)
}

/// ADR 0075: the summary is written on this Mac (the helper's on-device model; here the mock helper's), even
/// with a provider key saved: nothing reaches the provider. Search is full-text over the notes DB.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_on_device_summary_through_the_tauri_backend_and_full_text_search() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockLlm::default();
    let base = mock.start().await;
    let a = app(&tmp.path().join("data"), &base);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    call(
        &r,
        "settings",
        "secrets:set",
        json!({"name": "llm.openrouter", "value": "sk-or-test"}),
    )
    .await;

    let (id, p) = record_one(&a, &r, "Ship review").await;
    assert_eq!(p["steps"]["summary"]["status"], "done", "{p}");
    // No embeddings since the "ask" chat went: the index step is a no-op, never a failure.
    assert!(
        matches!(p["steps"]["index"]["status"].as_str(), Some("done" | "skipped")),
        "{p}"
    );
    let d = call(&r, "note", "note:get", json!({"id": id})).await;
    assert_eq!(d["summary"]["provider"], "apple-intelligence", "{d}");
    assert!(
        d["summary"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("mock on-device"),
        "{d}"
    );
    assert!(d["summary"].get("llmError").is_none_or(Value::is_null), "{d}");
    assert!(mock.seen().is_empty(), "no request to the provider");

    // Search is full-text over the notes DB (FTS): the transcript finds the meeting.
    let hits = call(&r, "note", "note:list", json!({"query": "mock helper"})).await;
    assert!(
        hits.as_array()
            .is_some_and(|h| h.iter().any(|m| m["meeting"]["id"] == id.as_str())),
        "{hits}"
    );
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_only_never_calls_the_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockLlm::default();
    let base = mock.start().await;
    let a = app(&tmp.path().join("data"), &base);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    call(
        &r,
        "settings",
        "secrets:set",
        json!({"name": "llm.openrouter", "value": "sk-or-test"}),
    )
    .await;
    call(
        &r,
        "settings",
        "config:set",
        json!({"patch": {"privacy": {"localOnly": true}}}),
    )
    .await;
    let (id, p) = record_one(&a, &r, "Private").await;
    assert_eq!(p["steps"]["summary"]["status"], "done", "{p}");
    let d = call(&r, "note", "note:get", json!({"id": id})).await;
    assert_ne!(d["summary"]["provider"], "openrouter", "{d}");
    assert!(mock.seen().is_empty(), "no request in local-only mode");
    a.runtime.kill();
}
