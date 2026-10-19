//! ADR 0191: "turn off my wifi" / "turn wi-fi back on" end to end on Linux, through the `ctrl:*` channels and the real
//! ctrl helper protocol to the `FakeHelper`. One `WorkspaceOp::SetWifi { on }` in the helper's own process turns the
//! Wi-Fi radio on or off (one direction per task, never the other as a fallback), the helper's read of the radio's
//! power verifies it, nothing is gated, no permission is asked, no app is activated and the user's frontmost app /
//! cursor / focus are untouched. Turning it off defers while the user is typing or clicking; already that way is done
//! with nothing acted. Networks by name or password, joining / leaving, the hotspot, toggles / restarts, other radios
//! and negations are not this.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Route, WorkspaceOp};
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

fn log_of(d: &std::path::Path, v: &Value, t: &str, text: &str) -> String {
    let store = Store::open(d).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    replay_check(&log, &Goal { task_id: t, text }).unwrap();
    log.to_jsonl().to_lowercase()
}

#[test]
fn turn_wifi_on_or_off_sets_the_radio_in_the_helper_and_verifies_it() {
    for (text, on) in [
        ("turn off my wifi", false),
        ("turn off wi-fi", false),
        ("turn the wifi off", false),
        ("disable wifi", false),
        ("switch off the wi fi", false),
        ("wifi off", false),
        ("every night at 11 turn off the wifi", false),
        ("turn off wifi on my mac", false),
        ("turn off the wifi on weekdays at 10pm", false),
        ("turn on my wifi", true),
        ("turn wi-fi back on", true),
        ("turn the wifi on", true),
        ("enable wifi", true),
        ("switch on wi-fi", true),
        ("every morning at 7 turn on the wifi", true),
        ("wifi on", true),
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.wifi = Some(!on);
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.wifi_power",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![Route::Workspace(WorkspaceOp::SetWifi { on })],
            "{text}"
        );
        assert_eq!(w.wifi, Some(on), "{text}");
        assert_eq!(w.muted, before.muted, "{text}: sound untouched");
        assert_eq!(w.dark, before.dark, "{text}: appearance untouched");
        assert_eq!(w.system_sleeps, 0, "{text}");
        assert!(!w.display_asleep, "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        assert_eq!(
            w.env.automation, before.env.automation,
            "{text}: no Apple Event grant asked or used"
        );
        let raw = log_of(d.path(), &v, &t, text);
        let mv = if on {
            "route.system.wifi_on"
        } else {
            "route.system.wifi_off"
        };
        assert!(raw.contains(mv), "{text}");
        for words in ["my wifi", "the wifi", "wi-fi", "turn off", "turn on", "my mac"] {
            assert!(!raw.contains(words), "{text}: task words in the log ({words})");
        }
    }
}

#[test]
fn only_the_bool_crosses_the_wire() {
    for on in [true, false] {
        assert_eq!(
            serde_json::to_value(Route::Workspace(WorkspaceOp::SetWifi { on })).unwrap(),
            json!({"route": "workspace", "op": "set_wifi", "on": on})
        );
    }
    assert_eq!(
        serde_json::to_value(qrtz_ctrl::helper::Check::WifiOn).unwrap(),
        json!({"check": "wifi_on"})
    );
    // Anything but a JSON bool is not a route (the helper reads `on` only; see CtrlWifiTests).
    for bad in [
        json!({"route": "workspace", "op": "set_wifi", "on": 1}),
        json!({"route": "workspace", "op": "set_wifi", "on": "true"}),
        json!({"route": "workspace", "op": "set_wifi"}),
    ] {
        assert!(serde_json::from_value::<Route>(bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn already_that_way_is_done_and_a_change_that_didnt_take_fails_the_verify() {
    for on in [true, false] {
        let text = if on { "turn on my wifi" } else { "turn off my wifi" };
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.wifi = Some(on);
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(fake.world().acted.is_empty(), "{text}: already so, nothing acted");

        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.wifi = Some(!on);
        w.no_effect = true;
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(v["state"]["status"], "done", "{text}: {v}");
        let w = fake.world();
        assert_eq!(w.wifi, Some(!on), "{text}");
        assert!(
            w.acted
                .iter()
                .all(|a| a.1 == Route::Workspace(WorkspaceOp::SetWifi { on })),
            "{text}: never the other direction: {:?}",
            w.acted
        );
    }
}

#[test]
fn turning_it_off_while_the_user_is_typing_defers_but_turning_it_on_does_not() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env.user_idle_ms = Some(300);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "turn off my wifi");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.wifi_power", "{v}");
    assert_eq!(v["state"]["status"], "deferred", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert_eq!(w.wifi, Some(true));

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env.user_idle_ms = Some(300);
    w.wifi = Some(false);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "turn on my wifi");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(fake.world().wifi, Some(true));
}

#[test]
fn no_wifi_or_an_admin_only_switch_is_not_done_and_nothing_else_is_tried() {
    for (none, refused) in [(true, false), (false, true)] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        if none {
            w.wifi = None;
        }
        w.wifi_refused = refused;
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, "turn off the wifi");
        let v = run_now(&s, &t);
        assert_ne!(v["state"]["status"], "done", "none {none} refused {refused}: {v}");
        let w = fake.world();
        assert_eq!(w.wifi, before.wifi);
        assert!(
            w.acted
                .iter()
                .all(|a| a.1 == Route::Workspace(WorkspaceOp::SetWifi { on: false })),
            "no other route: {:?}",
            w.acted
        );
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
    }
}

#[test]
fn other_network_tasks_are_not_this() {
    for text in [
        "connect to my home wifi",
        "join the wifi network",
        "disconnect from wifi",
        "forget the wifi network",
        "what's the wifi password",
        "share my wifi password",
        "turn on the personal hotspot",
        "toggle wifi",
        "restart my wifi",
        "turn the wifi off and on again",
        "don't turn off the wifi",
        "keep wifi on",
        "turn on wifi calling",
        "turn off bluetooth",
        "turn off airplane mode",
        "open wifi settings",
        "turn off my display",
        "mute my mac",
        "turn wifi",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.wifi_power",
            "{text}: {v}"
        );
        let w = fake.world();
        assert_eq!(w.wifi, Some(true), "{text}");
        assert!(
            !w.acted
                .iter()
                .any(|a| matches!(a.1, Route::Workspace(WorkspaceOp::SetWifi { .. }))),
            "{text}"
        );
    }
}
