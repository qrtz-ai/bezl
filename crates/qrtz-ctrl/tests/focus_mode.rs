//! ADR 0155 / 0157: "turn on do not disturb" / "turn off focus", end to end on Linux through the `ctrl:*` channels
//! to the `FakeHelper`. The route is the user's one "qrtz Set Focus" shortcut run through Shortcuts Events with a
//! mode from a closed set: without the shortcut the run stops with `focus.no_shortcut` (the view says "set up the
//! shortcut"), without Automation with `automation.denied` ("allow automation"), and with both it sets the Focus.
//! The user's frontmost app, cursor and focus are untouched either way.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{shortcut_allowed, FocusMode, Route, WorkspaceOp, FOCUS_SHORTCUT};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_790_000_000_000;

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

fn world() -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.watch_since_ms = Some(T0 - 60_000);
    w.day_start_ms = T0 - 3 * 3_600_000;
    w
}

#[test]
fn without_the_shortcut_the_run_stops_and_asks_for_it() {
    for text in [
        "turn on do not disturb",
        "dnd off",
        "turn on focus",
        "every weekday at 9am turn on do not disturb",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let st = &v["state"];
        assert_eq!(st["decisions"][0]["chosen"], "intent.focus_mode", "{text}: {v}");
        assert_eq!(st["status"], "asked-out", "{text}: {v}");
        assert_eq!(st["held"], "focus-no-shortcut", "{text}: {v}");
        assert!(
            st["ask"].is_null() && st["confirm"].is_null(),
            "{text}: no question, no card"
        );
        let raw = v.to_string();
        let w = fake.world();
        assert_eq!(w.focus, FocusMode::Off, "{text}: nothing set");
        assert_eq!(w.frontmost_pid, before.frontmost_pid, "{text}");
        assert_eq!(w.cursor, before.cursor, "{text}");
        assert_eq!(w.focused, before.focused, "{text}");
        assert!(
            !raw.contains("/usr/bin/shortcuts") && !raw.contains("osascript"),
            "{text}: no fallback"
        );

        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
        let raw = log.to_jsonl().to_lowercase();
        // The move id names the mode (`route.system.focus.do_not_disturb`); the task's own text stays out.
        assert!(
            !raw.contains(text),
            "{text}: the task's words stay out of the log"
        );
        assert!(!raw.contains("cmd+"), "{text}: no key event planned");
    }
}

#[test]
fn denied_automation_stops_with_allow_automation() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.focus_shortcut = true;
    w.shortcuts_denied = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "turn on do not disturb");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert_eq!(v["state"]["held"], "automation-denied", "{v}");
    assert_eq!(fake.world().focus, FocusMode::Off);
}

#[test]
fn with_the_shortcut_focus_goes_on_and_off_in_the_background() {
    for (text, mode, mv, label) in [
        (
            "turn on do not disturb",
            FocusMode::DoNotDisturb,
            "route.system.focus.do_not_disturb",
            "turn on do not disturb",
        ),
        (
            "start a work focus",
            FocusMode::Work,
            "route.system.focus.work",
            "turn on that focus",
        ),
        (
            "turn on sleep focus",
            FocusMode::Sleep,
            "route.system.focus.sleep",
            "turn on that focus",
        ),
        (
            "turn off do not disturb",
            FocusMode::Off,
            "route.system.focus.off",
            "turn focus off",
        ),
        (
            "dnd off",
            FocusMode::Off,
            "route.system.focus.off",
            "turn focus off",
        ),
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.focus_shortcut = true;
        if mode == FocusMode::Off {
            w.focus = FocusMode::DoNotDisturb;
        }
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let st = &v["state"];
        assert_eq!(st["status"], "done", "{text}: {v}");
        let after = fake.world();
        assert_eq!(after.focus, mode, "{text}");
        assert_eq!(
            after.acted.last().unwrap().1,
            Route::Workspace(WorkspaceOp::SetFocus { mode }),
            "{text}"
        );
        assert_eq!(after.frontmost_pid, before.frontmost_pid, "{text}");
        assert_eq!(after.cursor, before.cursor, "{text}");
        assert_eq!(after.focused, before.focused, "{text}");
        let m = st["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["node"] == "choose_move")
            .unwrap()
            .clone();
        assert_eq!(m["chosen"], mv, "{m}");
        assert_eq!(m["chosenLabel"], label, "{m}");
        let store = Store::open(d.path()).unwrap();
        let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
        log.verify().unwrap();
        replay_check(&log, &Goal { task_id: &t, text }).unwrap();
    }
}

#[test]
fn only_the_one_shortcut_name_and_the_closed_modes_get_through() {
    assert_eq!(FOCUS_SHORTCUT, "qrtz Set Focus");
    assert!(shortcut_allowed("qrtz Set Focus"));
    for n in [
        "qrtz set focus",
        "Set Focus",
        "qrtz Set Focus ",
        "",
        "rm -rf",
        "qrtz Set Focus\"; quit",
    ] {
        assert!(!shortcut_allowed(n), "{n:?}");
    }
    for m in FocusMode::ALL {
        assert_eq!(FocusMode::parse(m.as_str()), Some(m));
    }
    for bad in [
        "",
        "dnd",
        "do not disturb",
        "Do_Not_Disturb",
        "party",
        "work ",
        "on",
        "off\n",
        "\"; quit",
    ] {
        assert_eq!(FocusMode::parse(bad), None, "{bad:?}");
        let wire = json!({"route": "workspace", "op": "set_focus", "mode": bad});
        assert!(
            serde_json::from_value::<Route>(wire).is_err(),
            "{bad:?} never deserializes"
        );
    }
    let ok = json!({"route": "workspace", "op": "set_focus", "mode": "do_not_disturb"});
    assert_eq!(
        serde_json::from_value::<Route>(ok).unwrap(),
        Route::Workspace(WorkspaceOp::SetFocus {
            mode: FocusMode::DoNotDisturb
        })
    );
}

#[test]
fn other_phrasings_keep_their_intents() {
    for (text, intent) in [
        ("mute my mac", "intent.mute"),
        ("turn on dark mode", "intent.dark_mode"),
        ("hide safari", "intent.hide_app"),
        ("hide other apps", "intent.hide_others"),
        ("turn the volume up", "intent.change_volume"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
    }
}
