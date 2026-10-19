//! ADR 0197: "hide the menu bar" / "always show the menu bar" end to end on Linux, through the `ctrl:*` channels and
//! the real ctrl helper protocol to the `FakeHelper`. One System Events script (a faceless agent) sets the menu bar's
//! auto-hide, the helper's read of the global defaults' `_HIHideMenuBar` verifies it, nothing is gated (it's
//! reversible), no app is activated or hidden, and the user's frontmost app / cursor / focus are untouched. Already
//! the way the task wants is done with nothing sent (so no grant is asked). Without the System Events Automation
//! grant the run asks and nothing runs; the Dock, menu-bar items and full-screen modes are not this task.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Consent, Route, ScriptVerb, WorkspaceOp};
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
    FakeWorld::typical(2)
}

fn granted() -> FakeWorld {
    let mut w = world();
    w.env
        .automation
        .insert("com.apple.systemevents".into(), Consent::Granted);
    w
}

fn log_of(d: &std::path::Path, v: &Value, t: &str, text: &str) -> String {
    let store = Store::open(d).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    replay_check(&log, &Goal { task_id: t, text }).unwrap();
    log.to_jsonl().to_lowercase()
}

fn dock() -> FakeApp {
    FakeApp::new("com.apple.dock", "Dock", 90)
}

#[test]
fn hide_and_show_the_menu_bar_set_its_autohide_through_system_events_and_verify_it() {
    for (text, verb, after) in [
        ("hide the menu bar", ScriptVerb::SetMenuBarAutohide, true),
        ("auto-hide the menu bar", ScriptVerb::SetMenuBarAutohide, true),
        ("hide my menubar", ScriptVerb::SetMenuBarAutohide, true),
        ("turn on menu bar hiding", ScriptVerb::SetMenuBarAutohide, true),
        (
            "automatically hide and show the menu bar",
            ScriptVerb::SetMenuBarAutohide,
            true,
        ),
        (
            "every morning keep the menu bar hidden",
            ScriptVerb::SetMenuBarAutohide,
            true,
        ),
        ("show the menu bar", ScriptVerb::SetMenuBarShown, false),
        ("always show the menu bar", ScriptVerb::SetMenuBarShown, false),
        ("stop hiding the menu bar", ScriptVerb::SetMenuBarShown, false),
        ("turn off menu bar auto-hide", ScriptVerb::SetMenuBarShown, false),
        ("don't hide the menu bar", ScriptVerb::SetMenuBarShown, false),
        ("never hide the menu-bar", ScriptVerb::SetMenuBarShown, false),
        (
            "every friday keep the menu bar visible",
            ScriptVerb::SetMenuBarShown,
            false,
        ),
    ] {
        for with_dock in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let mut w = granted();
            w.menu_bar_autohide = !after;
            if with_dock {
                w.apps.push(dock());
            }
            let (s, fake) = rig(d.path(), w);
            let before_w = fake.world();
            let t = create(&s, text);
            let v = run_now(&s, &t);
            assert_eq!(v["state"]["status"], "done", "{text}: {v}");
            assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
            assert_eq!(
                v["state"]["decisions"][0]["chosen"], "intent.menu_bar_autohide",
                "{text}"
            );
            let w = fake.world();
            assert_eq!(
                w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
                vec![Route::Scripting {
                    app: "com.apple.systemevents".into(),
                    verb
                }],
                "{text}: one move"
            );
            assert_eq!(w.menu_bar_autohide, after, "{text}");
            assert_eq!(w.dock_autohide, before_w.dock_autohide, "{text}: dock untouched");
            assert_eq!(w.dock_edge, before_w.dock_edge, "{text}: dock edge untouched");
            assert_eq!(w.dark, before_w.dark, "{text}: appearance untouched");
            assert_eq!(w.frontmost_pid, before_w.frontmost_pid);
            assert_eq!(w.cursor, before_w.cursor);
            assert_eq!(w.focused, before_w.focused);
            assert!(w
                .apps
                .iter()
                .zip(before_w.apps.iter())
                .all(|(a, b)| a.running == b.running && a.hidden == b.hidden));
            let raw = log_of(d.path(), &v, &t, text);
            assert!(raw.contains("route.scripting.menu_bar_autohide_"), "{text}");
            assert!(!raw.contains("the menu bar"), "{text}: task words in the log");
        }
    }
}

#[test]
fn already_the_way_the_task_wants_is_done_with_nothing_sent_and_no_grant_asked() {
    for (text, already) in [("hide the menu bar", true), ("always show the menu bar", false)] {
        let d = tempfile::tempdir().unwrap();
        // No System Events grant: the pre-check alone answers, so nothing is asked.
        let mut w = world();
        w.menu_bar_autohide = already;
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(fake.world().acted.is_empty(), "{text}: {:?}", fake.world().acted);
        assert_eq!(fake.world().menu_bar_autohide, already);
    }
}

#[test]
fn without_the_system_events_grant_it_asks_and_nothing_runs() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "hide the menu bar");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert!(v.to_string().contains("com.apple.systemevents"), "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert!(!w.menu_bar_autohide);

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.menu_bar_autohide = true;
    w.env
        .automation
        .insert("com.apple.systemevents".into(), Consent::Denied);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "always show the menu bar");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty());
    assert!(fake.world().menu_bar_autohide);
}

#[test]
fn a_script_that_didnt_take_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.no_effect = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "hide the menu bar");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    assert!(!fake.world().menu_bar_autohide);
}

#[test]
fn menu_bar_items_full_screen_modes_and_the_dock_are_not_this_task() {
    for text in [
        "hide the wifi icon in the menu bar",
        "hide the clock in the menu bar",
        "show the battery percentage in the menu bar",
        "hide the menu bar in full screen",
        "hide the menu bar on the desktop",
        "show the menu bar background",
        "toggle the menu bar",
        "hide the dock and the menu bar",
        "hide notes and the menu bar",
        "open the menu bar settings",
        "hide slack",
        "show slack",
        "turn on dark mode",
        "mute my mac",
        "hide the dock",
        "always show the dock",
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = granted();
        w.apps.push(dock());
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.menu_bar_autohide",
            "{text}: {v}"
        );
        let w = fake.world();
        assert!(!w.menu_bar_autohide, "{text}");
        assert!(
            !w.acted.iter().any(|a| matches!(
                a.1,
                Route::Scripting {
                    verb: ScriptVerb::SetMenuBarAutohide | ScriptVerb::SetMenuBarShown,
                    ..
                }
            )),
            "{text}: {:?}",
            w.acted
        );
        if text.contains("menu bar") {
            // Nothing at all for these: never a hide or a hidden launch of an app named beside the menu bar (the
            // Dock included), never the Dock's auto-hide for half of "the dock and the menu bar".
            assert!(w.acted.is_empty(), "{text}: {:?}", w.acted);
            assert!(!w.dock_autohide, "{text}");
            assert!(
                !w.acted.iter().any(|a| matches!(
                    a.1,
                    Route::Workspace(WorkspaceOp::HideApps { .. } | WorkspaceOp::LaunchHidden { .. })
                )),
                "{text}"
            );
        }
    }
    for (text, intent) in [
        ("hide the dock", "intent.dock_autohide"),
        ("always show the dock", "intent.dock_autohide"),
        ("move the dock to the left", "intent.dock_edge"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = rig(d.path(), granted());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["decisions"][0]["chosen"], intent, "{text}: {v}");
    }
}

#[test]
fn only_route_app_and_verb_cross_the_wire() {
    let d = tempfile::tempdir().unwrap();
    let (s, _) = rig(d.path(), granted());
    let t = create(&s, "hide the menu bar");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    let raw = log_of(d.path(), &v, &t, "hide the menu bar");
    assert!(raw.contains("menu_bar_autohide_on"), "{raw}");
    assert!(!raw.contains("hide the menu"), "{raw}");
}

#[test]
fn the_wire_spellings_are_exact() {
    for (verb, wire) in [
        (ScriptVerb::SetMenuBarAutohide, "set_menu_bar_autohide"),
        (ScriptVerb::SetMenuBarShown, "set_menu_bar_shown"),
    ] {
        assert_eq!(serde_json::from_value::<ScriptVerb>(json!(wire)).unwrap(), verb);
        assert_eq!(serde_json::to_value(verb).unwrap(), json!(wire));
    }
    for bad in [
        "set_menubar_autohide",
        "set_menu_bar_hidden",
        "SetMenuBarAutohide",
        "set_menu_bar_autohide ",
        "set_menu_bar_toggle",
    ] {
        assert!(serde_json::from_value::<ScriptVerb>(json!(bad)).is_err(), "{bad}");
    }
    assert_eq!(
        serde_json::to_value(qrtz_ctrl::helper::Check::MenuBarAutohide).unwrap(),
        json!({"check": "menu_bar_autohide"})
    );
}
