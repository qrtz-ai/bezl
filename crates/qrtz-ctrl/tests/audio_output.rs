//! ADR 0198: "switch the sound to my airpods" / "play sound through the macbook speakers" end to end on Linux, through
//! the `ctrl:*` channels and the real ctrl helper protocol to the `FakeHelper`. One `WorkspaceOp::SetAudioOutput
//! { kind }` makes the one connected output device of that kind the Mac's sound output (the helper picks it; only the
//! closed kind crosses the wire), the helper's read of the current output's kind verifies it, nothing is gated, no
//! permission is asked, no app is activated, the volume / mute stay and the user's frontmost app / cursor / focus are
//! untouched. Already that kind is done with nothing acted; none or several of that kind, or a task that names no kind
//! ("the headphones"), is not done and nothing else is tried. The input side, volume, Bluetooth pairing, AirPlay and
//! negations are not this.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{AudioOutput, Route, WorkspaceOp};
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

/// A MacBook with AirPods, a USB headset and an HDMI display connected, sound on the built-in speakers.
fn world() -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.audio_devices = vec![
        AudioOutput::BuiltIn,
        AudioOutput::Bluetooth,
        AudioOutput::Usb,
        AudioOutput::Display,
    ];
    w.audio_current = Some(AudioOutput::BuiltIn);
    w
}

fn log_of(d: &std::path::Path, v: &Value, t: &str, text: &str) -> String {
    let store = Store::open(d).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    replay_check(&log, &Goal { task_id: t, text }).unwrap();
    log.to_jsonl().to_lowercase()
}

fn set(kind: AudioOutput) -> Route {
    Route::Workspace(WorkspaceOp::SetAudioOutput { kind })
}

#[test]
fn switch_the_sound_output_picks_the_one_device_of_that_kind_and_verifies_it() {
    use AudioOutput::*;
    for (text, kind) in [
        ("switch the sound to my airpods", Bluetooth),
        ("switch to my airpods", Bluetooth),
        ("use my airpods for sound", Bluetooth),
        ("play sound through my bluetooth headphones", Bluetooth),
        (
            "every morning at 9 switch the audio output to my airpods",
            Bluetooth,
        ),
        ("switch the sound on my mac to bluetooth", Bluetooth),
        (
            "switch the sound from the macbook speakers to my airpods",
            Bluetooth,
        ),
        ("switch the sound to the macbook speakers", BuiltIn),
        ("play sound through the built-in speakers", BuiltIn),
        ("change the audio output to the internal speakers", BuiltIn),
        ("switch the sound back to my mac", BuiltIn),
        ("switch the sound from my airpods to the laptop speakers", BuiltIn),
        ("use the usb speakers", Usb),
        ("set the sound output to my usb headset", Usb),
        ("switch the sound to my monitor", Display),
        ("play audio through the tv", Display),
        (
            "every weekday at 6pm switch the sound output to the display",
            Display,
        ),
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        if kind == BuiltIn {
            w.audio_current = Some(Bluetooth);
        }
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(v["state"]["status"], "done", "{text}: {v}");
        assert!(v["state"]["confirm"].is_null(), "{text}: reversible, no gate");
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.audio_output",
            "{text}"
        );
        let w = fake.world();
        assert_eq!(
            w.acted.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
            vec![set(kind)],
            "{text}"
        );
        assert_eq!(w.audio_current, Some(kind), "{text}");
        assert_eq!(w.muted, before.muted, "{text}: mute untouched");
        assert_eq!(w.volume, before.volume, "{text}: volume untouched");
        assert_eq!(w.wifi, before.wifi, "{text}");
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
        assert_eq!(w.cursor, before.cursor);
        assert_eq!(w.focused, before.focused);
        assert_eq!(
            w.env.automation, before.env.automation,
            "{text}: no Apple Event grant asked or used"
        );
        let raw = log_of(d.path(), &v, &t, text);
        assert!(
            raw.contains(&format!("route.system.audio_output_{}", kind.as_str())),
            "{text}"
        );
        for words in ["airpods", "macbook", "speakers", "monitor", "headset", "my mac"] {
            assert!(!raw.contains(words), "{text}: task words in the log ({words})");
        }
    }
}

#[test]
fn only_the_kind_crosses_the_wire() {
    for (kind, wire, code) in [
        (AudioOutput::BuiltIn, "built_in", 10),
        (AudioOutput::Bluetooth, "bluetooth", 20),
        (AudioOutput::Usb, "usb", 30),
        (AudioOutput::Display, "display", 40),
    ] {
        assert_eq!(
            serde_json::to_value(set(kind)).unwrap(),
            json!({"route": "workspace", "op": "set_audio_output", "kind": wire})
        );
        assert_eq!(kind.code(), code);
        assert_eq!(
            qrtz_ctrl::exec::audio_output_of_move(&format!("route.system.audio_output_{wire}")),
            Some(kind)
        );
    }
    assert_eq!(
        serde_json::to_value(qrtz_ctrl::helper::Check::AudioOutput).unwrap(),
        json!({"check": "audio_output"})
    );
    assert_eq!(
        qrtz_ctrl::exec::audio_output_of_move("route.system.audio_output_airplay"),
        None
    );
    assert_eq!(
        qrtz_ctrl::exec::audio_output_of_move("route.system.audio_output_"),
        None
    );
    // A device name, id or UID is never a route; neither is another kind or spelling.
    for bad in [
        json!({"route": "workspace", "op": "set_audio_output", "kind": "airplay"}),
        json!({"route": "workspace", "op": "set_audio_output", "kind": "Bluetooth"}),
        json!({"route": "workspace", "op": "set_audio_output", "kind": "builtin"}),
        json!({"route": "workspace", "op": "set_audio_output", "kind": "jared's airpods"}),
        json!({"route": "workspace", "op": "set_audio_output", "device": 73}),
        json!({"route": "workspace", "op": "set_audio_output"}),
    ] {
        assert!(serde_json::from_value::<Route>(bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn already_that_kind_is_done_and_a_switch_that_didnt_take_fails_the_verify() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.audio_current = Some(AudioOutput::Bluetooth);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "switch the sound to my airpods");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(fake.world().acted.is_empty(), "already so, nothing acted");

    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.no_effect = true;
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "switch the sound to my airpods");
    let v = run_now(&s, &t);
    assert_ne!(v["state"]["status"], "done", "{v}");
    let w = fake.world();
    assert_eq!(w.audio_current, Some(AudioOutput::BuiltIn));
    assert!(
        w.acted.iter().all(|a| a.1 == set(AudioOutput::Bluetooth)),
        "never another kind: {:?}",
        w.acted
    );
}

#[test]
fn switching_while_the_user_is_typing_is_not_deferred() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world();
    w.env.user_idle_ms = Some(300);
    let (s, fake) = rig(d.path(), w);
    let t = create(&s, "switch the sound to my airpods");
    let v = run_now(&s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(fake.world().audio_current, Some(AudioOutput::Bluetooth));
}

#[test]
fn none_or_several_of_that_kind_or_a_refusal_is_not_done_and_nothing_else_is_tried() {
    for (devices, refused) in [
        // AirPods not connected.
        (vec![AudioOutput::BuiltIn, AudioOutput::Usb], false),
        // Two Bluetooth outputs: the task can't say which.
        (
            vec![
                AudioOutput::BuiltIn,
                AudioOutput::Bluetooth,
                AudioOutput::Bluetooth,
            ],
            false,
        ),
        // Core Audio refuses the change.
        (vec![AudioOutput::BuiltIn, AudioOutput::Bluetooth], true),
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut w = world();
        w.audio_devices = devices.clone();
        w.audio_output_refused = refused;
        let (s, fake) = rig(d.path(), w);
        let before = fake.world();
        let t = create(&s, "switch the sound to my airpods");
        let v = run_now(&s, &t);
        assert_ne!(v["state"]["status"], "done", "{devices:?} {refused}: {v}");
        let w = fake.world();
        assert_eq!(w.audio_current, Some(AudioOutput::BuiltIn));
        assert!(
            w.acted.iter().all(|a| a.1 == set(AudioOutput::Bluetooth)),
            "no other route: {:?}",
            w.acted
        );
        assert_eq!(w.frontmost_pid, before.frontmost_pid);
    }
}

#[test]
fn a_task_that_names_no_one_kind_asks_and_switches_nothing() {
    for text in [
        "switch the sound to my headphones",
        "switch the sound to the speakers",
        "play sound through my airpods or the usb speakers",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_eq!(
            v["state"]["decisions"][0]["chosen"], "intent.audio_output",
            "{text}: {v}"
        );
        assert_ne!(v["state"]["status"], "done", "{text}: {v}");
        let w = fake.world();
        assert!(w.acted.is_empty(), "{text}: {:?}", w.acted);
        assert_eq!(w.audio_current, Some(AudioOutput::BuiltIn), "{text}");
    }
}

#[test]
fn other_sound_tasks_are_not_this() {
    for text in [
        "switch my microphone to the airpods",
        "use the usb mic for input",
        "set the volume of my airpods to 50",
        "turn the volume up",
        "mute my mac",
        "connect my airpods",
        "disconnect my airpods",
        "pair my bluetooth headphones",
        "turn off bluetooth",
        "switch on bluetooth",
        "turn on noise cancellation on my airpods",
        "play sound on all my speakers",
        "switch the sound to my apple tv",
        "airplay to the living room homepod",
        "change the alert sound",
        "don't switch the sound to my airpods",
        "open sound settings",
        "turn off my display",
        "move the dock to the left",
        "switch to dark mode",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (s, fake) = rig(d.path(), world());
        let t = create(&s, text);
        let v = run_now(&s, &t);
        assert_ne!(
            v["state"]["decisions"][0]["chosen"], "intent.audio_output",
            "{text}: {v}"
        );
        let w = fake.world();
        assert_eq!(w.audio_current, Some(AudioOutput::BuiltIn), "{text}");
        assert!(
            !w.acted
                .iter()
                .any(|a| matches!(a.1, Route::Workspace(WorkspaceOp::SetAudioOutput { .. }))),
            "{text}"
        );
    }
}
