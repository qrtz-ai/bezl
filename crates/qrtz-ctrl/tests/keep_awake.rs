//! ADR 0194: "keep my mac awake for 2 hours" / "let my mac sleep again" end to end on Linux, through the `ctrl:*`
//! channels and the real ctrl helper protocol to the `FakeHelper`. One `WorkspaceOp::KeepAwake { minutes, display }`
//! makes the helper hold one power assertion of its own with a timeout (the task's duration, an hour when it names
//! none, never more than 12 hours and never without an end); `AllowSleep` releases it. The helper's read of its own
//! assertion verifies both. Nothing is gated or deferred, no permission is asked, no app is activated, nothing is put
//! to sleep, and the user's frontmost app / cursor / focus are untouched. The log replays with no task words.
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

fn moves(w: &FakeWorld) -> Vec<Route> {
    w.acted.iter().map(|a| a.1.clone()).collect()
}

#[test]
fn keep_the_mac_awake_for_the_tasks_duration_or_an_hour() {
    for (text, minutes, display) in [
        ("keep my mac awake", 60, false),
        ("keep my mac awake for 2 hours", 120, false),
        ("keep the computer awake for 30 minutes", 30, false),
        ("stay awake for an hour", 60, false),
        ("keep my laptop awake for half an hour", 30, false),
        ("keep my mac awake for an hour and a half", 90, false),
        ("keep my mac awake for 90 mins", 90, false),
        ("keep my mac awake for three hours", 180, false),
        ("caffeinate my mac for 45 minutes", 45, false),
        ("don't let my mac sleep for 4 hours", 240, false),
        ("prevent my mac from sleeping for 20 minutes", 20, false),
        ("stop my mac from sleeping for an hour", 60, false),
        ("every weekday at 9 keep my mac awake for 8 hours", 480, false),
        ("keep my mac awake for 12 hours", 720, false),
        ("keep my screen on for 2 hours", 120, true),
        ("keep the display awake", 60, true),
        ("don't let the screen sleep for 15 minutes", 15, true),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.keep_awake",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            moves(&w),
            vec![Route::Workspace(WorkspaceOp::KeepAwake { minutes, display })],
            "{text}"
        );
        assert_eq!(w.awake, Some((minutes, display)), "{text}");
        assert_eq!(w.system_sleeps, 0, "{text}: nothing put to sleep");
        assert!(!w.display_asleep, "{text}");
        assert_eq!(w.wifi, before.wifi, "{text}");
        assert_eq!(w.muted, before.muted, "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        assert_eq!(w.env.automation, before.env.automation, "{text}: no Apple Event");
        let raw = log_of(d.path(), &v, &t, text);
        let mv = if display {
            "route.system.keep_awake_display"
        } else {
            "route.system.keep_awake"
        };
        assert!(raw.contains(mv), "{text}");
        assert!(raw.contains(&format!("minutes.{minutes}")), "{text}");
        for words in ["my mac", "awake for", "the screen", "hours", "caffeinate my"] {
            assert!(!raw.contains(words), "{text}: task words in the log ({words})");
        }
    }
}

#[test]
fn only_the_minutes_and_the_display_bool_cross_the_wire() {
    assert_eq!(
        serde_json::to_value(Route::Workspace(WorkspaceOp::KeepAwake {
            minutes: 90,
            display: true
        }))
        .unwrap(),
        json!({"route": "workspace", "op": "keep_awake", "minutes": 90, "display": true})
    );
    assert_eq!(
        serde_json::to_value(Route::Workspace(WorkspaceOp::AllowSleep)).unwrap(),
        json!({"route": "workspace", "op": "allow_sleep"})
    );
    assert_eq!(
        serde_json::to_value(qrtz_ctrl::helper::Check::KeptAwake).unwrap(),
        json!({"check": "kept_awake"})
    );
    for bad in [
        json!({"route": "workspace", "op": "keep_awake", "minutes": "60", "display": false}),
        json!({"route": "workspace", "op": "keep_awake", "minutes": -1, "display": false}),
        json!({"route": "workspace", "op": "keep_awake", "minutes": 60, "display": 0}),
        json!({"route": "workspace", "op": "keep_awake", "minutes": 60}),
    ] {
        assert!(serde_json::from_value::<Route>(bad.clone()).is_err(), "{bad}");
    }
    use qrtz_ctrl::exec::minutes_of;
    assert_eq!(minutes_of("minutes.60"), Some(60));
    assert_eq!(minutes_of("minutes.720"), Some(720));
    for bad in [
        "minutes.0",
        "minutes.721",
        "minutes.-5",
        "minutes.x",
        "level.60",
        "minutes.",
    ] {
        assert_eq!(minutes_of(bad), None, "{bad}");
    }
}

#[test]
fn a_duration_ctrl_wont_guess_is_asked_and_nothing_is_held() {
    for text in [
        "keep my mac awake for 24 hours",
        "keep my mac awake for 13 hours",
        "keep my mac awake for a couple of hours",
        "keep my mac awake for a few hours",
        "keep my mac awake for 2 hours or 3 hours",
        "keep my mac awake for 0 minutes",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.keep_awake",
            "{text}: {v}"
        );
        assert_eq!(v["state"]["status"], "asking", "{text}: {v}");
        let w = fake.world();
        assert!(w.acted.is_empty(), "{text}: {:?}", w.acted);
        assert_eq!(w.awake, None, "{text}");
    }
}

#[test]
fn let_it_sleep_again_releases_only_the_helpers_own_hold() {
    for text in [
        "let my mac sleep again",
        "allow my mac to sleep",
        "stop keeping my mac awake",
        "let the display sleep again",
        "every day at 6pm let my computer sleep",
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.awake = Some((120, false));
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.allow_sleep",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            moves(&w),
            vec![Route::Workspace(WorkspaceOp::AllowSleep)],
            "{text}"
        );
        assert_eq!(w.awake, None, "{text}");
        assert_eq!(w.system_sleeps, 0, "{text}: let it sleep is not sleep now");
        assert!(!w.display_asleep, "{text}");
        let raw = log_of(d.path(), &v, &t, text);
        assert!(raw.contains("route.system.allow_sleep"), "{text}");
    }
    // Nothing held: done with nothing acted.
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "let my mac sleep again");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty());
}

#[test]
fn never_deferred_and_a_refused_or_ineffective_hold_is_not_done() {
    // The user typing right now doesn't hold it back: nothing on screen changes.
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env.user_idle_ms = Some(300);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "keep my mac awake for an hour");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(fake.world().awake, Some((60, false)));

    for (refused, no_effect) in [(true, false), (false, true)] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.awake_refused = refused;
        w.no_effect = no_effect;
        let (s, fake) = rig(d.path(), w);
        let t = create(&s, "keep my mac awake for an hour");
        let v = run_now(&s, &t);
        assert_ne!(v["state"]["status"], "done", "refused {refused}: {v}");
        let w = fake.world();
        assert_eq!(w.awake, None);
        assert!(
            w.acted
                .iter()
                .all(|a| matches!(a.1, Route::Workspace(WorkspaceOp::KeepAwake { .. }))),
            "no other route: {:?}",
            w.acted
        );
        assert_eq!(w.system_sleeps, 0);
    }
}

#[test]
fn sleep_tasks_and_other_keep_tasks_are_not_this() {
    for (text, not) in [
        ("put my mac to sleep", "both"),
        ("turn off my display", "both"),
        ("start the screensaver", "both"),
        ("keep my mac awake forever", "both"),
        ("never let my mac sleep", "both"),
        ("keep my mac awake until 5pm", "both"),
        ("keep my mac awake while it downloads", "both"),
        ("change my sleep settings", "both"),
        ("wake my mac at 7", "both"),
        ("lock my mac", "both"),
        ("keep wifi on", "both"),
        ("turn off my wifi", "both"),
        ("mute my mac", "both"),
        ("don't let my mac sleep", "allow_sleep"),
        ("stop my mac from sleeping", "allow_sleep"),
        ("let my mac sleep", "keep_awake"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        let chosen = v["state"]["decisions"][0]["chosen"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if not != "allow_sleep" {
            assert_ne!(chosen, "intent.keep_awake", "{text}: {v}");
        }
        if not != "keep_awake" {
            assert_ne!(chosen, "intent.allow_sleep", "{text}: {v}");
        }
        let w = fake.world();
        assert!(
            !w.acted.iter().any(|a| matches!(
                a.1,
                Route::Workspace(WorkspaceOp::KeepAwake { .. } | WorkspaceOp::AllowSleep)
            )) || not != "both",
            "{text}"
        );
    }
    // "let my mac sleep" is not a sleep now: it never reaches the power manager's sleep.
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "let my mac sleep");
    let _ = run_now(&s, &t);
    assert_eq!(fake.world().system_sleeps, 0);
}
