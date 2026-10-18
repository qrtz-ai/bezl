//! ADR 0145: "toggle dark mode" end to end on Linux, through the `ctrl:*` channels and the real ctrl helper protocol
//! to the `FakeHelper`. The driver reads the appearance just before, takes the other one through the same System
//! Events script as ADR 0137, verifies it by the read-back, and logs the read-before value as the verify's before.
//! Nothing is gated, no app is activated and the user's frontmost app / cursor / focus are untouched.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Consent, Route, ScriptVerb};
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

#[test]
fn toggle_flips_the_appearance_read_just_before_and_verifies_it() {
    for (text, dark_before, verb) in [
        ("toggle dark mode", false, ScriptVerb::SetDarkMode),
        ("toggle dark mode", true, ScriptVerb::SetLightMode),
        ("every evening flip dark mode", false, ScriptVerb::SetDarkMode),
        ("toggle light mode", true, ScriptVerb::SetLightMode),
        ("toggle the dark mode on my mac", true, ScriptVerb::SetLightMode),
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = granted();
        w.dark = dark_before;
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        assert_eq!(v["state"]["decisions"][0]["chosen"], "intent.dark_mode", "{text}");
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![Route::Scripting {
                app: "com.apple.systemevents".into(),
                verb
            }],
            "{text}"
        );
        assert_eq!(w.dark, !dark_before, "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        let raw = log_of(d.path(), &v, &t, text);
        assert!(raw.contains("route.scripting.dark_mode_toggle"), "{text}");
        assert!(
            !raw.contains("dark mode") && !raw.contains("light mode"),
            "{text}: task words in the log"
        );
    }
}

#[test]
fn on_and_off_still_win_without_toggle_and_no_grant_asks_with_nothing_changed() {
    // "turn on dark mode" stays the fixed on route (ADR 0137), never the toggle, so a dark Mac stays dark.
    let d = tempfile::tempdir().unwrap();
    let mut w = granted();
    w.dark = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "turn on dark mode");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().dark);
    assert!(!log_of(d.path(), &v, &t, "turn on dark mode").contains("dark_mode_toggle"));

    // No System Events grant: the toggle asks (foreground prompt only) and nothing is switched.
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world());
    let t = create(&s, "toggle dark mode");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    let w = fake.world();
    assert!(w.acted.is_empty(), "{:?}", w.acted);
    assert!(!w.dark);
    let _ = Consent::Denied;
}
