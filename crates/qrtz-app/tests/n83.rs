//! N83: the Tauri backend runs the full step chain, SearchSteps (index) → LlmSteps (the configured provider) →
//! CoreSteps, end to end through `build_app` and the router with the Rust mock helper. The provider is a local
//! mock OpenRouter (never the real network) reached through the `QRTZ_LLM_BASE_URL` dev knob.
use async_trait::async_trait;
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use qrtz_model::LifecycleError;
use qrtz_search::{Embedder, LazyEmbedder};
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
    fn push(&self, r: String) {
        self.replies.lock().unwrap().push_back(r);
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

fn completion(content: &Value) -> String {
    let body = json!({"choices": [{"message": {"content": content.to_string()}}]}).to_string();
    http(200, "application/json", &body)
}

/// A deterministic 4-d embedder (the index step and the backfill write vectors with its id).
struct Fake;
#[async_trait]
impl Embedder for Fake {
    fn id(&self) -> &str {
        "fake-4d"
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        Ok(texts.iter().map(|_| vec![0.5, 0.5, 0.5, 0.5]).collect())
    }
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

fn app(dir: &Path, llm_base: &str, embedder: Arc<LazyEmbedder>) -> BuiltApp {
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
        steps: None,
        embedder: Some(embedder),
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
    let rt = a.runtime.clone();
    until("helper", || rt.session().helper.as_str() == "simulated").await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_openrouter_summary_and_the_index_step_through_the_tauri_backend() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockLlm::default();
    let base = mock.start().await;
    mock.push(completion(&json!({
        "summary": "the team agreed to ship on friday.",
        "actionItems": [{"text": "ship it", "owner": "bobs"}],
        "qa": []
    })));
    let a = app(
        &tmp.path().join("data"),
        &base,
        LazyEmbedder::ready_with(Arc::new(Fake)),
    );
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    a.search.start();
    // The key the user saved in settings (the app's 0600 store) is what the provider gets.
    call(
        &r,
        "settings",
        "secrets:set",
        json!({"name": "llm.openrouter", "value": "sk-or-test"}),
    )
    .await;

    let (id, p) = record_one(&a, &r, "Ship review").await;
    assert_eq!(p["steps"]["summary"]["status"], "done", "{p}");
    assert_eq!(p["steps"]["index"]["status"], "done", "{p}");
    let d = call(&r, "note", "note:get", json!({"id": id})).await;
    assert_eq!(d["summary"]["provider"], "openrouter", "{d}");
    assert_eq!(d["summary"]["summary"], "the team agreed to ship on friday.");
    assert_eq!(d["summary"]["actionItems"][0]["text"], "ship it");
    assert!(d["summary"].get("llmError").is_none_or(Value::is_null), "{d}");

    let seen = mock.seen();
    assert_eq!(seen.len(), 1, "one summary request");
    let (line, head, body) = &seen[0];
    assert!(line.starts_with("POST /chat/completions"), "{line}");
    assert!(head.contains("authorization: bearer sk-or-test"), "{head}");
    assert!(head.contains("x-title: qrtz"), "{head}");
    let req: Value = serde_json::from_str(body).unwrap();
    let prompt = req["messages"].to_string();
    assert!(
        prompt.contains("Hello from the mock helper."),
        "the transcript is sent: {prompt}"
    );
    assert!(prompt.contains("<transcript-"), "inside the N69 fence");

    // The index step wrote vectors with the embedder's id: nothing is left for the backfill.
    let missing =
        a.db.call(|db| db.notes().meetings_missing_vectors("fake-4d"))
            .await
            .unwrap();
    assert!(missing.is_empty(), "{missing:?}");
    a.search.shutdown();
    assert!(a.search.backfill.is_cancelled());
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_redirect_is_not_followed_and_the_summary_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockLlm::default();
    let base = mock.start().await;
    // N81 through the app: a 307 would re-POST the transcript and the key to the Location.
    let elsewhere = MockLlm::default();
    let other = elsewhere.start().await;
    for _ in 0..8 {
        mock.push(format!(
            "HTTP/1.1 307 X\r\nlocation: {other}/chat/completions\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        ));
    }
    let a = app(&tmp.path().join("data"), &base, LazyEmbedder::none());
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
    let (id, p) = record_one(&a, &r, "Redirected").await;
    // A redirect is a configuration error: not retried, and the meeting still gets the extractive summary.
    let sum = &p["steps"]["summary"];
    assert_eq!(
        (sum["status"].as_str(), sum["fallback"].as_bool()),
        (Some("failed"), Some(true)),
        "{p}"
    );
    assert_eq!(sum["error"]["code"], "E_PROVIDER");
    assert_eq!(sum["error"]["retryable"], false);
    assert!(sum["error"]["message"]
        .as_str()
        .unwrap()
        .contains("redirect not followed"));
    assert!(!sum.to_string().contains("sk-or-test"), "no key in the error");
    let d = call(&r, "note", "note:get", json!({"id": id})).await;
    assert!(d["summary"]["summary"].is_string(), "extractive fallback: {d}");
    assert_ne!(d["summary"]["provider"], "openrouter", "{d}");
    assert!(
        elsewhere.seen().is_empty(),
        "the redirect target never got the key or transcript"
    );
    // No embedder: the index step is skipped, never failed (full-text search still works).
    assert!(
        matches!(p["steps"]["index"]["status"].as_str(), Some("done" | "skipped")),
        "{p}"
    );
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_only_never_calls_the_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockLlm::default();
    let base = mock.start().await;
    let a = app(&tmp.path().join("data"), &base, LazyEmbedder::none());
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_boot_backfill_embeds_old_meetings_but_not_one_pending_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let mock = MockLlm::default();
    let base = mock.start().await;
    // First run: no model yet, so nothing is embedded.
    let (keep, gone) = {
        let a = app(&data, &base, LazyEmbedder::none());
        let r = Router::new(Shared(a.service.clone())).validate_responses(true);
        a.service.start_helper();
        a.runtime.recover().await.unwrap();
        let (keep, _) = record_one(&a, &r, "Keep").await;
        let (gone, _) = record_one(&a, &r, "Gone").await;
        a.runtime.kill();
        (keep, gone)
    };
    // Next boot: the model finishes loading only after "Gone" went into its undo window.
    let gate = Arc::new(tokio::sync::Notify::new());
    let g = gate.clone();
    let loader: qrtz_search::embedder::Loader = Box::new(move || {
        let g = g.clone();
        Box::pin(async move {
            g.notified().await;
            Ok(Arc::new(Fake) as Arc<dyn Embedder>)
        })
    });
    let a = app(&data, &base, LazyEmbedder::new(loader));
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    a.search.start();
    call(&r, "note", "note:delete", json!({"id": gone, "deferMs": 60_000})).await;
    gate.notify_one();
    let db = a.db.clone();
    let mut missing = vec![];
    for _ in 0..400 {
        missing = db
            .call(|db| db.notes().meetings_missing_vectors("fake-4d"))
            .await
            .unwrap();
        if !missing.contains(&keep) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!missing.contains(&keep), "the old meeting was embedded");
    assert_eq!(
        missing,
        vec![gone],
        "a meeting in its undo window is not embedded (N17)"
    );
    a.search.shutdown();
    a.runtime.kill();
}

/// Counts embed calls.
struct Counting(Arc<std::sync::atomic::AtomicUsize>);
#[async_trait]
impl Embedder for Counting {
    fn id(&self) -> &str {
        "fake-4d"
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LifecycleError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(texts.iter().map(|_| vec![0.5, 0.5, 0.5, 0.5]).collect())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n86_the_backfill_yields_to_a_live_recording() {
    use std::sync::atomic::Ordering;
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let mock = MockLlm::default();
    let base = mock.start().await;
    // A library with an un-embedded meeting.
    let old = {
        let a = app(&data, &base, LazyEmbedder::none());
        let r = Router::new(Shared(a.service.clone())).validate_responses(true);
        a.service.start_helper();
        a.runtime.recover().await.unwrap();
        let (id, _) = record_one(&a, &r, "Old").await;
        a.runtime.kill();
        id
    };
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let gate = Arc::new(tokio::sync::Notify::new());
    let (g, c) = (gate.clone(), calls.clone());
    let loader: qrtz_search::embedder::Loader = Box::new(move || {
        let (g, c) = (g.clone(), c.clone());
        Box::pin(async move {
            g.notified().await;
            Ok(Arc::new(Counting(c)) as Arc<dyn Embedder>)
        })
    });
    let a = app(&data, &base, LazyEmbedder::new(loader));
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    a.search.start();
    let rt = a.runtime.clone();
    until("helper", || rt.session().helper.as_str() == "simulated").await;
    call(&r, "note", "note:start", json!({"title": "Live"})).await;
    assert!(a.live.busy(), "a recording holds the live-work counter");
    // The model becomes ready mid-recording: the backfill must not embed anything until it stops.
    gate.notify_one();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no embed call while recording");
    call(&r, "note", "note:stop", json!(null)).await;
    let rt2 = a.runtime.clone();
    until("done", || rt2.session().phase.as_str() == "done").await;
    // Once idle (the steps release it too) the backfill runs and embeds the old meeting.
    let db = a.db.clone();
    let mut missing = vec![old.clone()];
    for _ in 0..400 {
        missing = db
            .call(|db| db.notes().meetings_missing_vectors("fake-4d"))
            .await
            .unwrap();
        if !missing.contains(&old) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!missing.contains(&old), "embedded after the recording");
    assert!(calls.load(Ordering::SeqCst) > 0);
    assert!(!a.live.busy(), "nothing left holding it");
    a.search.shutdown();
    a.runtime.kill();
}
