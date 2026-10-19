//! ADR 0187: "unminimize slack" / "restore my minimized windows" / "minimize slack", end to end on Linux through the
//! `ctrl:*` channels to the `FakeHelper`. Before this they scored as `open_app` (a hidden launch of a running app that
//! reported done with nothing restored) or as a close-windows question. Now each is its own intent whose one move is
//! a hold: asked out with `no-route`, nothing sent to the helper, until a Mac probe shows an `AXMinimized` write
//! raises no window over the user's, activates nothing and moves no focus. The ladder refuses the write itself too.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{judge, AxOp, ContentRef, Refusal, Route, Verdict};
use qrtz_ctrl::policy::FORBIDDEN_AX_SET_ATTRIBUTES;
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

/// Notes frontmost, Safari, Finder, Slack and TextEdit running; Chrome not running.
fn world() -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.apps
        .push(FakeApp::new("com.tinyspeck.slackmacgap", "Slack", 500));
    w.apps.push(FakeApp::new("com.apple.TextEdit", "TextEdit", 400));
    w.watch_since_ms = Some(T0 - 60_000);
    w.day_start_ms = MIDNIGHT;
    w
}

/// Every phrasing of one intent holds before anything reaches the helper, and leaves the user's session as it was.
fn holds(text: &str, intent: &str, mv: &str) {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let before = fake.world();
    let t = create(&s, text);
    let v = run_now(&s, &t);
    let st = &v["state"];
    assert_eq!(st["decisions"][0]["chosen"], intent, "{text}: {v}");
    let moves: Vec<&Value> = st["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| &d["chosen"])
        .collect();
    assert_eq!(moves, vec![intent, mv], "{text}: no app question while held");
    assert_eq!(st["status"], "asked-out", "{text}: {v}");
    assert_eq!(st["held"], "no-route", "{text}: {v}");
    assert!(
        st["ask"].is_null() && st["confirm"].is_null(),
        "{text}: no question, no card"
    );
    let w = fake.world();
    assert!(w.acted.is_empty(), "{text}: nothing sent: {:?}", w.acted);
    assert_eq!(w.frontmost_pid, before.frontmost_pid, "{text}");
    assert_eq!(w.cursor, before.cursor, "{text}");
    assert_eq!(w.focused, before.focused, "{text}");
    assert_eq!(
        w.apps.iter().filter(|a| a.running).count(),
        before.apps.iter().filter(|a| a.running).count(),
        "{text}: nothing launched or quit"
    );

    let store = Store::open(d.path()).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    replay_check(&log, &Goal { task_id: &t, text }).unwrap();
    let raw = log.to_jsonl().to_lowercase();
    assert!(raw.contains(mv), "{text}");
    for never in [
        "launch_hidden",
        "unhide_apps",
        "scripting.close_window",
        "axminimized",
        "cmd+",
    ] {
        assert!(!raw.contains(never), "{text}: {never} in the log");
    }
    assert!(
        !raw.contains(text),
        "{text}: the task's words stay out of the log"
    );
}

#[test]
fn unminimize_is_recognised_and_held_with_nothing_sent() {
    for text in [
        "unminimize slack",
        "un-minimize slack",
        "unminimise textedit",
        "deminimize textedit",
        "restore slack windows",
        "restore slack's windows",
        "restore textedit",
        "restore my minimized windows",
        "show slack's minimized windows",
        "show slack minimized windows",
        "show minimized windows",
        "open slack's minimized window",
        "bring slack back from the dock",
        "get my windows out of the dock",
        "unminimize all my windows",
        "every morning unminimize slack",
    ] {
        holds(text, "intent.unminimize_app", "route.windows.unminimize_hold");
    }
}

#[test]
fn minimize_is_recognised_and_held_with_nothing_sent() {
    for text in [
        "minimize slack",
        "minimise textedit",
        "minimize all my windows",
        "minimize slack windows",
        "every evening minimize textedit",
    ] {
        holds(text, "intent.minimize_app", "route.windows.minimize_hold");
    }
}

#[test]
fn other_phrasings_keep_their_intents() {
    for (text, intent) in [
        ("show textedit", "intent.show_app"),
        ("unhide textedit", "intent.show_app"),
        ("hide slack", "intent.hide_app"),
        ("open slack", "intent.open_app"),
        ("close slack windows", "intent.close_windows"),
        ("hide the dock", "intent.dock_autohide"),
        ("quit slack", "intent.quit_app"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
    }
    // The Dock's minimize settings and "restore" of a folder are neither window intent.
    for text in [
        "minimize windows into application icon",
        "use the genie effect to minimize",
        "restore my downloads",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let first = v["state"]["decisions"][0]["chosen"].as_str().unwrap_or("");
        assert!(
            first != "intent.unminimize_app" && first != "intent.minimize_app",
            "{text}: {v}"
        );
    }
}

#[test]
fn the_ladder_refuses_an_axminimized_write_both_ways() {
    let env = world().env;
    for role in ["AXWindow", "AXStandardWindow"] {
        let r = Route::Ax {
            app: "com.tinyspeck.slackmacgap".into(),
            role: role.into(),
            element: ContentRef("e:window".into()),
            op: AxOp::SetAttribute {
                attribute: "AXMinimized".into(),
            },
        };
        assert_eq!(
            judge(&r, &env),
            Verdict::Refused {
                why: Refusal::ForbiddenAxAttribute {
                    attribute: "AXMinimized".into()
                }
            },
            "{role}"
        );
    }
    assert!(FORBIDDEN_AX_SET_ATTRIBUTES.contains(&"AXMinimized"));
}
