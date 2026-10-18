//! Slice 7b (ADR 0114): "close all my tabs" end to end on Linux, through the `ctrl:*` channels and the real ctrl
//! helper protocol to the `FakeHelper`. Plain tabs close without asking, pinned tabs are never touched, tabs with
//! unsaved form input wait behind the confirm gate (deny keeps them open, approve closes them), the user's screen is
//! never touched, no tab title reaches disk, and the log replays.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{Route, ScriptVerb};
use qrtz_ctrl::log::{Event, EventLog};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::IanaZone;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_790_000_000_000;
const TEXT: &str = "every day close all my tabs";

struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn mono_ns(&self) -> u64 {
        self.0.fetch_add(1000, Ordering::SeqCst)
    }
    fn wall_ms(&self) -> u64 {
        T0
    }
}

/// Safari (default browser, in the background) with 3 plain tabs, 2 pinned and the given unsaved-input tabs.
fn world(unsaved: &[&str]) -> FakeWorld {
    let mut w = FakeWorld::typical(3);
    let safari = w
        .apps
        .iter_mut()
        .find(|a| a.bundle_id == "com.apple.Safari")
        .unwrap();
    safari.pinned_tabs = vec!["Pinned Mail Inbox".into(), "Pinned Calendar".into()];
    safari.unsaved_tabs = unsaved.iter().map(|t| (*t).to_string()).collect();
    w
}

fn rig(dir: &Path, w: FakeWorld) -> (CtrlService, Arc<FakeHelper>) {
    let fake = Arc::new(FakeHelper::new(w));
    let client = Arc::new(HelperClient::new(Box::new(fake.clone())));
    let s = CtrlService::open_with_zone(
        dir,
        Box::new(FakeClock(AtomicU64::new(0))),
        Box::new(IanaZone(Los_Angeles)),
    )
    .with_helper(client);
    (s, fake)
}

fn run(s: &CtrlService) -> Value {
    let id = s
        .call(
            "ctrl:create-task",
            json!({"text": TEXT, "schedule": {"kind": "daily", "hour": 18, "minute": 0}}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = s.call("ctrl:run-now", json!({"id": id})).unwrap();
    s.call("ctrl:run", json!({"runId": r["id"]})).unwrap()
}

fn confirm(s: &CtrlService, v: &Value, answer: &str) -> Value {
    s.call(
        "ctrl:confirm",
        json!({"runId": v["run"]["id"], "gateId": v["state"]["confirm"]["gateId"], "answer": answer}),
    )
    .unwrap()
}

fn kinds(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

fn count(v: &Value, kind: &str) -> usize {
    kinds(v).iter().filter(|k| *k == kind).count()
}

fn safari(f: &FakeHelper) -> qrtz_ctrl::helper::FakeApp {
    f.world()
        .apps
        .into_iter()
        .find(|a| a.bundle_id == "com.apple.Safari")
        .unwrap()
}

fn untouched(w: &FakeWorld) {
    let t = FakeWorld::typical(0);
    assert_eq!(w.frontmost_pid, t.frontmost_pid, "frontmost app changed");
    assert_eq!(w.cursor, t.cursor, "cursor moved");
    assert_eq!(w.focused, t.focused, "focus changed");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            v.extend(walk(&p));
        } else {
            v.push(p);
        }
    }
    v
}

fn no_titles_on_disk(dir: &Path, titles: &[&str]) {
    for f in walk(dir) {
        let text = std::fs::read_to_string(&f).unwrap_or_default().to_lowercase();
        for t in titles {
            assert!(!text.contains(&t.to_lowercase()), "{t} reached {}", f.display());
        }
    }
}

fn replays(dir: &Path, v: &Value) -> EventLog {
    let store = Store::open(dir).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    log.verify().unwrap();
    let task_id = v["run"]["taskId"].as_str().unwrap();
    replay_check(&log, &Goal { task_id, text: TEXT }).unwrap();
    let back = EventLog::from_jsonl(log.run_id(), &log.to_jsonl()).unwrap();
    assert_eq!(back.state(), log.state());
    log
}

fn script(verb: ScriptVerb) -> Route {
    Route::Scripting {
        app: "com.apple.Safari".into(),
        verb,
    }
}

#[test]
fn plain_tabs_close_without_asking_and_pinned_tabs_stay() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world(&[]));
    let v = run(&s);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(count(&v, "confirm.requested"), 0, "no unsaved input: no gate");
    let a = safari(&fake);
    assert_eq!(a.tabs, 0, "plain tabs closed");
    assert_eq!(a.pinned_tabs.len(), 2, "pinned tabs never closed");
    let w = fake.world();
    assert_eq!(
        w.acted,
        vec![("a0".to_string(), script(ScriptVerb::CloseSavedTabs))]
    );
    untouched(&w);
    assert_eq!(v["state"]["violations"], 0);
    assert_eq!(
        v["state"]["verified"],
        json!([{"actionId": "a0", "outcome": "passed"}])
    );
    replays(d.path(), &v);
    no_titles_on_disk(d.path(), &["pinned mail inbox", "pinned calendar"]);
}

#[test]
fn unsaved_input_tabs_wait_behind_the_gate_and_deny_keeps_them_open() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world(&["Visa Application Form"]));
    let v = run(&s);
    let st = &v["state"];
    assert_eq!(st["status"], "needs-confirm", "{v}");
    let a = safari(&fake);
    assert_eq!(a.tabs, 0, "the plain tabs closed before the gate");
    assert_eq!(a.pinned_tabs.len(), 2);
    assert_eq!(a.unsaved_tabs.len(), 1, "the unsaved tab waits");
    // The card lists only the held tab, by its (in-memory) title, marked unsaved.
    let items = st["confirm"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["label"], "visa application form");
    assert_eq!(items[0]["unsaved"], true);
    assert_eq!(items[0]["kind"], "tab");
    assert_eq!(s.take_confirm_notices().len(), 1);

    let v = confirm(&s, &v, "deny");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(v["state"]["confirm"]["answer"], "denied");
    let a = safari(&fake);
    assert_eq!(a.unsaved_tabs.len(), 1, "deny keeps it open");
    assert_eq!(a.pinned_tabs.len(), 2);
    let w = fake.world();
    assert_eq!(w.acted.len(), 1, "only the plain-tab close ran");
    untouched(&w);
    replays(d.path(), &v);
    no_titles_on_disk(d.path(), &["visa application", "pinned mail inbox"]);
}

#[test]
fn approve_closes_the_unsaved_tabs_after_a_recheck_and_still_skips_pinned() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world(&["Visa Application Form", "Draft Comment"]));
    let v = run(&s);
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    let v = confirm(&s, &v, "approve");
    assert_eq!(v["state"]["status"], "done", "{v}");
    let a = safari(&fake);
    assert!(a.unsaved_tabs.is_empty(), "approved: closed");
    assert_eq!(a.tabs, 0);
    assert_eq!(a.pinned_tabs.len(), 2, "pinned tabs still skipped");
    let w = fake.world();
    assert_eq!(
        w.acted,
        vec![
            ("a0".to_string(), script(ScriptVerb::CloseSavedTabs)),
            ("a1".to_string(), script(ScriptVerb::CloseUnpinnedTabs)),
        ]
    );
    untouched(&w);
    assert_eq!(v["state"]["violations"], 0);
    assert_eq!(v["state"]["confirm"]["used"], true);
    let k = kinds(&v);
    let pos = |name: &str| k.iter().rposition(|x| x == name).unwrap();
    assert!(pos("confirm.resolved") < pos("confirm.reverified"));
    assert!(pos("confirm.reverified") < pos("action.requested"));
    let log = replays(d.path(), &v);
    no_titles_on_disk(
        d.path(),
        &["visa application", "draft comment", "pinned calendar"],
    );

    // A log that closes the unsaved tabs without the gate doesn't replay.
    let mut forged = EventLog::new("forged");
    for r in log
        .records()
        .iter()
        .take_while(|r| !matches!(r.event, Event::ConfirmRequested { .. }))
    {
        forged.append(r.t_mono_ns, r.t_wall_ms, r.event.clone());
    }
    forged.append(
        0,
        T0,
        Event::ActionRequested {
            action_id: "a1".into(),
            routes: vec![script(ScriptVerb::CloseUnpinnedTabs)],
        },
    );
    let task_id = v["run"]["taskId"].as_str().unwrap();
    assert!(replay_check(&forged, &Goal { task_id, text: TEXT }).is_err());
}

#[test]
fn a_new_unsaved_tab_before_the_answer_asks_again() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = rig(d.path(), world(&["Visa Application Form"]));
    let v = run(&s);
    assert_eq!(v["state"]["status"], "needs-confirm");
    fake.0
        .lock()
        .unwrap()
        .apps
        .iter_mut()
        .find(|a| a.bundle_id == "com.apple.Safari")
        .unwrap()
        .unsaved_tabs
        .push("Bank Transfer".into());
    let v = confirm(&s, &v, "approve");
    assert_eq!(
        v["state"]["status"], "needs-confirm",
        "the grown world re-asks: {v}"
    );
    assert_eq!(v["state"]["confirm"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        safari(&fake).unsaved_tabs.len(),
        2,
        "nothing closed on the stale answer"
    );
    let v = confirm(&s, &v, "approve");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(safari(&fake).unsaved_tabs.is_empty());
    assert_eq!(safari(&fake).pinned_tabs.len(), 2);
    replays(d.path(), &v);
}

#[test]
fn a_helper_that_cannot_enumerate_asks_for_every_tab() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world(&[]);
    w.apps
        .iter_mut()
        .find(|a| a.bundle_id == "com.apple.Safari")
        .unwrap()
        .no_preview = true;
    let (s, fake) = rig(d.path(), w);
    let v = run(&s);
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    assert_eq!(v["state"]["confirm"]["complete"], false);
    assert!(
        fake.world().acted.is_empty(),
        "can't tell pinned or unsaved: nothing closes unasked"
    );
}
