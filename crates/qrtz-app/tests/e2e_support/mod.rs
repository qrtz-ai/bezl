//! Phase 5 harness (docs/e2e-parity.md): the Tauri backend exactly as `apps/desktop/src-tauri` boots it
//! (`build_app` → `start_helper` → `recover` → `search.start`, quit = search shutdown + helper `shutdown` line),
//! driven through the IPC router like the renderer, with a real helper child process and local mock servers.
//! Every wait polls a condition against a deadline; nothing sleeps for a fixed time to let work finish.
#![allow(dead_code)]
use qrtz_app::ports::{Notice, ShellHooks, TalkPort};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::storage_port::StorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Default deadline for a condition (debug build, tests in parallel).
pub const DEADLINE: Duration = Duration::from_secs(30);

/// Poll `f` every 25 ms until it yields `Some`, or panic after `DEADLINE`.
pub async fn wait<T, F, Fut>(what: &str, f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    wait_for(what, DEADLINE, f).await
}

pub async fn wait_for<T, F, Fut>(what: &str, deadline: Duration, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let end = Instant::now() + deadline;
    loop {
        if let Some(v) = f().await {
            return v;
        }
        if Instant::now() > end {
            panic!("timed out after {deadline:?} waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Which helper child the app supervises.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Helper {
    /// `qrtz-mock-helper` (crates/qrtz-helper-bridge): what `QRTZ_MOCK_HELPER=1` runs in the Tauri app.
    Mock,
    /// `examples/e2e_helper.rs`: the TS mock's scripted standup / revoke / insert / crash knobs.
    Scripted,
    /// No capture backend (`QRTZ_SIMULATE_MISSING_HELPER`): health `missing`.
    Missing,
}

fn built(cargo_args: &[&str], rel: &[&str]) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let mut p = exe.parent().unwrap().parent().unwrap().to_path_buf();
    for r in rel {
        p = p.join(r);
    }
    let st = std::process::Command::new(env!("CARGO"))
        .args(cargo_args)
        .status()
        .unwrap();
    assert!(st.success(), "cargo {cargo_args:?}");
    p
}

pub fn mock_helper_bin() -> PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        built(
            &[
                "build",
                "-q",
                "-p",
                "qrtz-helper-bridge",
                "--bin",
                "qrtz-mock-helper",
            ],
            &["qrtz-mock-helper"],
        )
    })
    .clone()
}

pub fn scripted_helper_bin() -> PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        built(
            &["build", "-q", "-p", "qrtz-app", "--example", "e2e_helper"],
            &["examples", "e2e_helper"],
        )
    })
    .clone()
}

/// OS notifications and HUD calls the shell received.
#[derive(Default)]
pub struct RecShell {
    pub notices: Mutex<Vec<Notice>>,
    pub hud: Mutex<Vec<bool>>,
}
impl ShellHooks for RecShell {
    fn set_hud_expanded(&self, e: bool) -> bool {
        self.hud.lock().unwrap().push(e);
        e
    }
    fn notify(&self, n: Notice) {
        self.notices.lock().unwrap().push(n);
    }
}

/// Clipboard, export dir and backup path (the dialogs) of the shell.
#[derive(Default)]
pub struct RecStorage {
    pub clipboard: Mutex<Vec<String>>,
    pub export_dir: Mutex<Option<PathBuf>>,
    pub backup: Mutex<Option<PathBuf>>,
}
impl RecStorage {
    pub fn clip(&self) -> Option<String> {
        self.clipboard.lock().unwrap().last().cloned()
    }
}
impl StorageShell for RecStorage {
    fn write_clipboard(&self, text: &str) {
        self.clipboard.lock().unwrap().push(text.to_owned());
    }
    fn save_path(&self, filename: &str) -> Option<PathBuf> {
        self.export_dir.lock().unwrap().as_ref().map(|d| d.join(filename))
    }
    fn backup_save_path(&self, _n: &str) -> Option<PathBuf> {
        self.backup.lock().unwrap().clone()
    }
    fn backup_open_path(&self) -> Option<PathBuf> {
        self.backup.lock().unwrap().clone()
    }
}

pub struct Opts {
    pub helper: Helper,
    pub env: Vec<(String, String)>,
    pub retry_scale: f64,
    /// `QRTZ_FAULT_STEP` (`DevKnobs::fault_step`): that step's first attempt per meeting fails.
    pub fault_step: Option<qrtz_model::StepId>,
    pub talk: Option<Arc<dyn TalkPort>>,
}
impl Default for Opts {
    fn default() -> Self {
        Self {
            helper: Helper::Mock,
            env: vec![],
            retry_scale: 0.001,
            fault_step: None,
            talk: None,
        }
    }
}
impl Opts {
    pub fn helper(mut self, h: Helper) -> Self {
        self.helper = h;
        self
    }
    pub fn env(mut self, k: &str, v: &str) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }
    pub fn fault_step(mut self, s: qrtz_model::StepId) -> Self {
        self.fault_step = Some(s);
        self
    }
    pub fn retry_scale(mut self, s: f64) -> Self {
        self.retry_scale = s;
        self
    }
    pub fn talk(mut self, t: Arc<dyn TalkPort>) -> Self {
        self.talk = Some(t);
        self
    }
    /// The standup scenario at `speed` (scripted helper).
    pub fn standup(self, speed: &str) -> Self {
        self.helper(Helper::Scripted)
            .env("QRTZ_MOCK_SCENARIO", "standup")
            .env("QRTZ_MOCK_SPEED", speed)
    }
}

/// One launch of the app on a data dir.
pub struct App {
    pub data: PathBuf,
    pub app: BuiltApp,
    pub r: Router<Shared>,
    pub shell: Arc<RecShell>,
    pub storage: Arc<RecStorage>,
    quit: bool,
}

impl App {
    /// Boot like `backend.rs::setup`: helper, then recovery, then search, then `note:changed *`.
    pub async fn launch(data: &Path, o: Opts) -> App {
        Self::launch_with(data, o, Arc::new(RecStorage::default())).await
    }

    pub async fn launch_with(data: &Path, o: Opts, storage: Arc<RecStorage>) -> App {
        let shell = Arc::new(RecShell::default());
        let helper = match o.helper {
            Helper::Missing => None,
            h => {
                let bin = if h == Helper::Mock {
                    mock_helper_bin()
                } else {
                    scripted_helper_bin()
                };
                let mut spawn = ProcessSpawn::helper(bin);
                spawn.env = o.env.clone();
                Some((spawn, HelperAvailability::Simulated))
            }
        };
        std::fs::create_dir_all(data).unwrap();
        let app = build_app(AppBuild {
            info: AppInfo {
                name: "qrtz".into(),
                version: "0.1.0".into(),
                platform: "linux".into(),
                arch: "x64".into(),
                shell: "tauri e2e".into(),
            },
            data_dir: data.to_path_buf(),
            helper,
            supervisor: SupervisorOptions::default(),
            shell: shell.clone(),
            storage_shell: storage.clone(),
            talk: o.talk,
            speech: None,
            steps: None,
            knobs: DevKnobs {
                retry_scale: Some(o.retry_scale),
                fault_step: o.fault_step,
                ..Default::default()
            },
        })
        .unwrap();
        let r = Router::new(Shared(app.service.clone())).validate_responses(true);
        app.service.start_helper();
        let _ = app.runtime.recover().await;
        app.service.events.emit("note:changed", json!({"id": "*"}));
        App {
            data: data.to_path_buf(),
            app,
            r,
            shell,
            storage,
            quit: false,
        }
    }

    /// Wait until every port saw the helper running (N91).
    pub async fn helper_ready(&self) {
        let svc = self.app.service.clone();
        wait("helper ready", || {
            let ok = svc.helper_ready();
            async move { ok.then_some(()) }
        })
        .await
    }

    pub async fn raw(&self, sub: &str, ch: &str, payload: Value) -> Value {
        self.r
            .dispatch(json!({"v":1,"id":"e2e","channel":ch,"version":1,"subApp":sub,"payload":payload}))
            .await
    }

    pub async fn call(&self, sub: &str, ch: &str, payload: Value) -> Value {
        let v = self.raw(sub, ch, payload).await;
        assert_eq!(v["ok"], true, "{ch}: {v}");
        v["data"].clone()
    }

    pub async fn note(&self, ch: &str, payload: Value) -> Value {
        self.call("note", ch, payload).await
    }

    pub async fn settings(&self, ch: &str, payload: Value) -> Value {
        self.call("settings", ch, payload).await
    }

    pub async fn session(&self) -> Value {
        self.note("note:session", Value::Null).await
    }

    /// Poll `note:session` until `pred` holds; returns that view.
    pub async fn until_session(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        wait(what, || async {
            let s = self.session().await;
            pred(&s).then_some(s)
        })
        .await
    }

    pub async fn until_phase(&self, phase: &str) -> Value {
        self.until_session(&format!("phase {phase}"), |s| s["phase"] == phase)
            .await
    }

    /// Poll `note:pipeline` until no step is pending/running/retrying.
    pub async fn until_settled(&self, id: &str) -> Value {
        wait(&format!("pipeline of {id} settled"), || async {
            let p = self.note("note:pipeline", json!({"id": id})).await;
            let busy = ["finalize", "summary", "index"].iter().any(|s| {
                matches!(
                    p["steps"][s]["status"].as_str(),
                    Some("pending" | "running" | "retrying")
                )
            });
            (!p.is_null() && !busy).then_some(p)
        })
        .await
    }

    pub async fn until_step(&self, id: &str, step: &str, status: &str) -> Value {
        wait(&format!("{step} {status}"), || async {
            let p = self.note("note:pipeline", json!({"id": id})).await;
            (p["steps"][step]["status"] == status).then_some(p)
        })
        .await
    }

    pub async fn talk(&self, ch: &str, payload: Value) -> Value {
        self.call("talk", ch, payload).await
    }

    pub async fn talk_state(&self) -> Value {
        self.talk("talk:state", Value::Null).await
    }

    /// Poll `talk:state` until `pred` holds; returns that view.
    pub async fn until_talk(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        wait(what, || async {
            let s = self.talk_state().await;
            pred(&s).then_some(s)
        })
        .await
    }

    pub async fn talk_history(&self) -> Vec<Value> {
        self.talk("talk:history", Value::Null)
            .await
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// One dictation: start, stream until "friday" is heard, stop, wait for the history entry; returns it.
    pub async fn dictate(&self) -> Value {
        let before = self.talk_history().await.len();
        self.talk("talk:start", Value::Null).await;
        self.until_talk("heard friday", |v| talk_text(v).contains("friday"))
            .await;
        self.talk("talk:stop", Value::Null).await;
        self.until_talk("talk idle", |v| v["phase"] == "idle").await;
        wait("history entry", || async {
            let h = self.talk_history().await;
            (h.len() > before).then(|| h[0].clone())
        })
        .await
    }

    pub fn helper_pid_alive(&self) -> bool {
        self.app.service.helper().is_some()
    }

    /// Quit like `backend.rs::shutdown`.
    pub async fn quit(mut self) {
        self.shutdown().await;
    }

    async fn shutdown(&mut self) {
        if self.quit {
            return;
        }
        self.quit = true;
        if let Some(h) = self.app.service.helper() {
            let bye =
                qrtz_helper_bridge::encode_host_message(&json!({"type": "shutdown"})).unwrap_or_default();
            h.shutdown(bye, Duration::from_millis(1500)).await;
        }
        self.app.runtime.kill();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if !self.quit {
            if let Some(h) = self.app.service.helper() {
                h.stop();
            }
            self.app.runtime.kill();
        }
    }
}

/// Record with the current helper until `enough` holds for `note:live-transcript`, stop, wait for done + settled.
pub async fn record_until(a: &App, title: Option<&str>, enough: impl Fn(&Value) -> bool) -> (String, Value) {
    let s = a
        .note("note:start", title.map_or(json!({}), |t| json!({ "title": t })))
        .await;
    let id = s["meetingId"].as_str().expect("meetingId").to_owned();
    wait("live transcript", || async {
        let t = a.note("note:live-transcript", json!({})).await;
        enough(&t).then_some(())
    })
    .await;
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    let p = a.until_settled(&id).await;
    (id, p)
}

/// What the pill shows: committed text + the live partial.
pub fn talk_text(v: &Value) -> String {
    format!(
        "{} {}",
        v["text"].as_str().unwrap_or(""),
        v["partial"].as_str().unwrap_or("")
    )
    .trim()
    .to_owned()
}

pub fn segments(t: &Value) -> usize {
    t["segments"].as_array().map_or(0, Vec::len)
}

pub fn has_text(t: &Value, needle: &str) -> bool {
    t["segments"].as_array().is_some_and(|s| {
        s.iter()
            .any(|x| x["text"].as_str().unwrap_or("").contains(needle))
    })
}

// ---------------------------------------------------------------- seeding (QRTZ_DEV_SEED has no Rust port)

const SPEAKERS: [&str; 5] = [
    "Ada Lovelace",
    "Grace Hopper",
    "Alan Turing",
    "Katherine Johnson",
    "Linus Torvalds",
];
const PHRASES: [&str; 8] = [
    "Let us review the quarterly roadmap and the launch checklist.",
    "The latency numbers on the streaming pipeline look good this week.",
    "Can you take the action item to follow up with the design team by Friday?",
    "I think we should ship the onboarding flow behind a feature flag first.",
    "We still need a decision on pricing for the team plan.",
    "Search over transcripts is fast even for long meetings.",
    "I will write the release notes and send them out tomorrow.",
    "Does anyone have concerns about the migration plan?",
];

/// `syntheticSegments(minutes)` (packages/core/src/note/large.ts): one ~4.8 s line per slot, rotating speakers.
pub fn synthetic_segments(minutes: u64) -> Vec<qrtz_model::Segment> {
    let step = 4800u64;
    (0..minutes * 60_000 / step)
        .map(|i| {
            serde_json::from_value(json!({
                "id": format!("syn-{i}"),
                "startMs": i * step,
                "endMs": i * step + step - 300,
                "text": format!("{} (point {i})", PHRASES[(i as usize * 7) % PHRASES.len()]),
                "speakerName": SPEAKERS[(i as usize * 3 + (i as usize >> 4)) % SPEAKERS.len()],
                "source": "live",
            }))
            .unwrap()
        })
        .collect()
}

fn seed_db(data: &Path, f: impl FnOnce(&qrtz_storage::NoteRepo)) {
    std::fs::create_dir_all(data).unwrap();
    let db = qrtz_storage::Db::open(data.join("qrtz.db")).unwrap();
    f(&qrtz_storage::NoteRepo::new(db.connection()));
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// `dev.seedMany`: `count` one-minute meetings "Sync #count" (newest) … "Sync #1", an hour apart.
pub fn seed_many(data: &Path, count: u64) {
    let now = now_ms();
    let segs = synthetic_segments(1);
    seed_db(data, |repo| {
        for i in 0..count {
            let id = format!("00000000-0000-4000-9000-{i:012}");
            let started = now - (i + 1) * 3_600_000;
            let m: qrtz_model::Meeting = serde_json::from_value(json!({
                "id": id,
                "title": format!("Sync #{}", count - i),
                "platform": if i % 2 == 1 { "zoom" } else { "google-meet" },
                "startedAt": started,
                "endedAt": started + 60_000,
                "durationMs": 60_000,
                "status": "done",
                "participants": ["Ada Lovelace", "Alan Turing"],
            }))
            .unwrap();
            repo.upsert_meeting(&m).unwrap();
            repo.replace_segments(&id, qrtz_model::SegmentSource::Live, &segs)
                .unwrap();
        }
    });
}

pub const LARGE_ID: &str = "00000000-0000-4000-8000-000000000120";

/// `dev.seedLarge`: a `minutes`-long "All-hands" meeting with its extractive summary.
pub fn seed_large(data: &Path, minutes: u64) {
    let segs = synthetic_segments(minutes);
    let started = now_ms() - 3 * 3_600_000;
    seed_db(data, |repo| {
        let mut people: Vec<String> = segs.iter().filter_map(|s| s.speaker_name.clone()).collect();
        people.sort();
        people.dedup();
        let m: qrtz_model::Meeting = serde_json::from_value(json!({
            "id": LARGE_ID,
            "title": format!("All-hands ({minutes} min)"),
            "platform": "google-meet",
            "startedAt": started,
            "endedAt": started + minutes * 60_000,
            "durationMs": minutes * 60_000,
            "status": "done",
            "participants": people,
        }))
        .unwrap();
        repo.upsert_meeting(&m).unwrap();
        repo.replace_segments(LARGE_ID, qrtz_model::SegmentSource::Live, &segs)
            .unwrap();
        let s = qrtz_llm::extractive_summary(&segs, None, &[], None);
        repo.save_summary(LARGE_ID, &s, now_ms() as _).unwrap();
    });
}
