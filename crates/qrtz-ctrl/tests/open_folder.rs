//! ADR 0166: "open my downloads" / "open the documents folder" end to end on Linux through the `ctrl:*` channels and
//! the ctrl helper protocol to the `FakeHelper`. One `WorkspaceOp::OpenFolder` of a folder from the closed set (the
//! helper resolves it from the system's own location and opens it in the running Finder with `activates = false`; no
//! key event, no activation, no gate: it's reversible), verified by `Check::FolderOpen`. The Finder not running asks
//! out (a cold start comes forward), a folder already shown is done with nothing acted, the user busy in the Finder
//! defers, and no path from the task's text ever reaches the helper.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient, Wire};
use qrtz_ctrl::ladder::{Route, UserFolder, WorkspaceOp, FINDER};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const T0: u64 = 1_790_000_000_000;
const MIDNIGHT: u64 = T0 - 3 * 3_600_000;

struct FakeClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl Clock for FakeClock {
    fn mono_ns(&self) -> u64 {
        self.mono.fetch_add(1000, Ordering::SeqCst)
    }
    fn wall_ms(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
}
struct ClockHandle(Arc<FakeClock>);
impl Clock for ClockHandle {
    fn mono_ns(&self) -> u64 {
        self.0.mono_ns()
    }
    fn wall_ms(&self) -> u64 {
        self.0.wall_ms()
    }
}

fn rig(dir: &std::path::Path, w: FakeWorld) -> (CtrlService, Arc<FakeHelper>) {
    let fake = Arc::new(FakeHelper::new(w));
    let clock = Arc::new(FakeClock {
        wall: AtomicU64::new(T0),
        mono: AtomicU64::new(0),
    });
    let client = Arc::new(HelperClient::new(Box::new(fake.clone())));
    let s = CtrlService::open_with_zone(dir, Box::new(ClockHandle(clock)), Box::new(IanaZone(Los_Angeles)))
        .with_helper(client);
    (s, fake)
}

fn create(s: &CtrlService, text: &str) -> String {
    s.call(
        "ctrl:create-task",
        json!({"text": text, "schedule": {"kind": "once", "at": null}}),
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn run_now(s: &CtrlService, task: &str) -> Value {
    let r = s.call("ctrl:run-now", json!({"id": task})).unwrap();
    s.call("ctrl:run", json!({"runId": r["id"]})).unwrap()
}

/// Notes frontmost, Safari, Finder, Slack, TextEdit (hidden) and qrtz running; Chrome not running.
fn world() -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.apps
        .push(FakeApp::new("com.tinyspeck.slackmacgap", "Slack", 500));
    let mut te = FakeApp::new("com.apple.TextEdit", "TextEdit", 400);
    te.hidden = true;
    w.apps.push(te);
    w.apps.push(FakeApp::new("ai.qrtz.desktop", "qrtz", 700));
    w.watch_since_ms = Some(T0 - 60_000);
    w.day_start_ms = MIDNIGHT;
    w
}

/// Every line the host sends, kept for the "no path reaches the helper" check.
struct Recording {
    inner: Arc<FakeHelper>,
    lines: Arc<Mutex<Vec<String>>>,
}
impl Wire for Recording {
    fn roundtrip(&self, line: &str) -> Result<String, String> {
        self.lines.lock().unwrap().push(line.to_string());
        self.inner.roundtrip(line)
    }
}

fn rig_recording(
    dir: &std::path::Path,
    w: FakeWorld,
) -> (CtrlService, Arc<FakeHelper>, Arc<Mutex<Vec<String>>>) {
    let fake = Arc::new(FakeHelper::new(w));
    let lines = Arc::new(Mutex::new(Vec::new()));
    let clock = Arc::new(FakeClock {
        wall: AtomicU64::new(T0),
        mono: AtomicU64::new(0),
    });
    let client = Arc::new(HelperClient::new(Box::new(Recording {
        inner: fake.clone(),
        lines: lines.clone(),
    })));
    let s = CtrlService::open_with_zone(dir, Box::new(ClockHandle(clock)), Box::new(IanaZone(Los_Angeles)))
        .with_helper(client);
    (s, fake, lines)
}

fn finder_windows(f: &FakeHelper) -> i64 {
    f.world()
        .apps
        .iter()
        .find(|a| a.bundle_id == FINDER)
        .map(|a| a.windows)
        .unwrap_or(0)
}

#[test]
fn open_my_folder_shows_it_in_the_finder_without_a_gate_key_or_activation() {
    for (text, folder) in [
        ("open my downloads", UserFolder::Downloads),
        ("open downloads", UserFolder::Downloads),
        ("open my downloads folder every morning", UserFolder::Downloads),
        ("open the documents folder", UserFolder::Documents),
        ("show my desktop folder", UserFolder::Desktop),
        ("go to my documents", UserFolder::Documents),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let st = &v["state"];
        assert_eq!(st["status"], "done", "{text}: {v}");
        assert!(st["confirm"].is_null(), "{text}: reversible, no card");
        let w = fake.world();
        let routes: Vec<&Route> = w.acted.iter().map(|(_, r)| r).collect();
        assert_eq!(
            routes,
            vec![&Route::Workspace(WorkspaceOp::OpenFolder { folder })],
            "{text}: exactly one workspace open of that folder"
        );
        assert_eq!(w.open_folders, vec![folder], "{text}");
        assert_eq!(finder_windows(&fake), 2, "{text}: one new finder window");
        assert_eq!(
            w.frontmost_pid, before.frontmost_pid,
            "{text}: the user's app stays frontmost (activates = false)"
        );
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);

        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        assert!(raw.contains("intent.open_folder"), "{text}: picked at the intent");
        assert!(raw.contains(&format!("folder.{}", folder.as_str())));
        assert!(raw.contains("route.workspace.open_folder"));
        assert!(
            raw.contains("\"op\":\"open_folder\""),
            "{text}: the act is the workspace open"
        );
        assert!(raw.contains("\"passed\""), "{text}: verified: {raw}");
        assert!(!raw.contains("cmd+"), "{text}: no key event planned");
        assert!(!raw.contains("launch_hidden"), "{text}: nothing launched");
        assert!(!raw.contains("route.files."), "{text}: nothing tidied");
    }
}

#[test]
fn the_finder_not_running_asks_out_and_nothing_is_opened() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    for a in w.apps.iter_mut().filter(|a| a.bundle_id == FINDER) {
        a.running = false;
    }
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "open my downloads");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.open_folder", "{v}");
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "nothing sent: {:?}", w.acted);
    assert!(w.open_folders.is_empty());
    assert!(
        !w.apps.iter().any(|a| a.bundle_id == FINDER && a.running),
        "the finder isn't started (a cold start comes forward)"
    );
}

#[test]
fn a_folder_already_shown_is_done_with_nothing_acted() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.open_folders = vec![UserFolder::Downloads];
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "open my downloads");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert_eq!(finder_windows(&fake), 1, "no second window");
    // Another folder still opens.
    let t = create(&s, "open my documents");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(
        fake.world().open_folders,
        vec![UserFolder::Downloads, UserFolder::Documents]
    );
}

#[test]
fn the_user_busy_in_the_finder_defers_and_nothing_is_opened() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.frontmost_pid = 50; // the Finder
    w.env.user_idle_ms = Some(300);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "open my downloads");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.open_folder", "{v}");
    assert_eq!(v["state"]["status"], "deferred", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert!(w.open_folders.is_empty());
    assert_eq!(w.frontmost_pid, 50);
}

#[test]
fn no_path_from_the_task_ever_reaches_the_helper() {
    let text = "open my downloads folder /etc/passwd ~/secret-plans";
    let d = tempfile::tempdir().unwrap();
    let (s, fake, lines) = rig_recording(d.path(), world());
    let t = create(&s, text);
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(
        fake.world().acted.iter().map(|(_, r)| r).collect::<Vec<_>>(),
        vec![&Route::Workspace(WorkspaceOp::OpenFolder {
            folder: UserFolder::Downloads
        })]
    );
    let lines = lines.lock().unwrap();
    let acts: Vec<&String> = lines.iter().filter(|l| l.contains("\"ctrl.act\"")).collect();
    assert_eq!(acts.len(), 1, "{acts:?}");
    let act: Value = serde_json::from_str(acts[0]).unwrap();
    assert_eq!(
        act["route"],
        json!({"route": "workspace", "op": "open_folder", "folder": "downloads"}),
        "only the enum crosses the wire"
    );
    assert_eq!(act["content"], json!({}), "no content ref to resolve");
    for l in lines.iter() {
        for word in ["/etc", "passwd", "secret", "~/", "/users/", "/home/"] {
            assert!(!l.to_lowercase().contains(word), "{word} reached the helper: {l}");
        }
    }
    // The wire itself refuses anything off the closed set.
    for bad in [
        "Downloads",
        "../downloads",
        "/Users/jared/Downloads",
        "screenshots",
        "",
    ] {
        let wire = json!({"route": "workspace", "op": "open_folder", "folder": bad});
        assert!(serde_json::from_value::<Route>(wire).is_err(), "{bad:?}");
    }
}

#[test]
fn other_phrasings_keep_their_intents() {
    for (text, intent) in [
        ("clean up my downloads", "intent.clean_folder"),
        ("tidy my desktop", "intent.clean_folder"),
        ("delete old files in my downloads", "intent.clean_folder"),
        ("open slack", "intent.open_app"),
        ("show textedit", "intent.show_app"),
        ("open nytimes.com", "intent.open_site"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
    }
    // "show the desktop" is macOS's Show Desktop, not a Finder window: never opened here.
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "show the desktop");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["decisions"][0]["chosen"], "intent.open_folder", "{v}");
    assert!(fake.world().open_folders.is_empty());
}

/// Run 40 (ADR 0166 amendment 1): Pictures, Music and Movies join the closed set. Pictures / Movies count with "my" or
/// "folder"; Music only with "folder" (the Music app is what "open my music" means), and then the Music app named by
/// that same word isn't another app.
fn world_with_music() -> FakeWorld {
    let mut w = world();
    w.apps.push(FakeApp::new("com.apple.Music", "Music", 800));
    w
}

#[test]
fn the_media_folders_open_in_the_finder_like_the_others() {
    for (text, folder) in [
        ("open my pictures", UserFolder::Pictures),
        ("open the pictures folder", UserFolder::Pictures),
        ("every friday open my pics", UserFolder::Pictures),
        ("open my movies", UserFolder::Movies),
        ("show my videos folder", UserFolder::Movies),
        ("open my music folder", UserFolder::Music),
        ("open the music folder every morning", UserFolder::Music),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake, lines) = rig_recording(d.path(), world_with_music());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let st = &v["state"];
        assert_eq!(st["decisions"][0]["chosen"], "intent.open_folder", "{text}: {v}");
        assert_eq!(st["status"], "done", "{text}: {v}");
        assert!(st["confirm"].is_null(), "{text}: reversible, no card");
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|(_, r)| r).collect::<Vec<_>>(),
            vec![&Route::Workspace(WorkspaceOp::OpenFolder { folder })],
            "{text}"
        );
        assert_eq!(w.open_folders, vec![folder], "{text}");
        assert_eq!(
            w.frontmost_pid, before.frontmost_pid,
            "{text}: nothing comes forward"
        );
        assert!(
            !w.apps
                .iter()
                .any(|a| a.bundle_id == "com.apple.Music" && a.windows != 1),
            "{text}: the Music app is left alone"
        );
        let lines = lines.lock().unwrap();
        let act: Value = serde_json::from_str(
            lines
                .iter()
                .find(|l| l.contains("\"ctrl.act\""))
                .expect("one act"),
        )
        .unwrap();
        assert_eq!(
            act["route"],
            json!({"route": "workspace", "op": "open_folder", "folder": folder.as_str()}),
            "{text}: only the enum crosses the wire"
        );
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        assert!(log.to_jsonl().contains("\"passed\""), "{text}: verified");
    }
}

#[test]
fn music_without_folder_is_the_music_app_and_photos_is_the_photos_app() {
    for (text, not) in [
        ("open music", "intent.open_folder"),
        ("open my music", "intent.open_folder"),
        ("open pictures", "intent.open_folder"),
        ("open my photos", "intent.open_folder"),
        ("clean up my pictures folder", "intent.open_folder"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world_with_music());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(v["state"]["decisions"][0]["chosen"], not, "{text}: {v}");
        assert!(fake.world().open_folders.is_empty(), "{text}");
    }
    let d = tempfile::tempdir().unwrap();
    let (s, _) = rig(d.path(), world_with_music());
    let t = create(&s, "open my music");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.open_app", "{v}");
    // The wire takes the new spellings and still nothing else.
    for ok in ["pictures", "music", "movies"] {
        let wire = json!({"route": "workspace", "op": "open_folder", "folder": ok});
        assert!(serde_json::from_value::<Route>(wire).is_ok(), "{ok}");
    }
    for bad in ["Pictures", "photos", "videos", "~/Music", "public", "library"] {
        let wire = json!({"route": "workspace", "op": "open_folder", "folder": bad});
        assert!(serde_json::from_value::<Route>(wire).is_err(), "{bad:?}");
    }
}
