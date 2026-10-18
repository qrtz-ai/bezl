//! Slice 6 (ADR 0111): the confirm gate end to end on Linux, through the `ctrl:*` channels and the real ctrl helper
//! protocol to the `FakeHelper`. Deny never acts, approve acts exactly once, a remembered approval is reused only
//! inside its scope, a grown scope or a changed world asks again, the log replays and a fork from before the answer
//! asks again, and a scheduled run waits for the user (then expires) without acting.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::gate::{needs_confirm, CONFIRM_TTL_MS};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::log::{Event, EventLog};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::{IanaZone, LocalZone};
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

fn textedit(docs: &[(&str, bool)]) -> FakeApp {
    let mut a = FakeApp::new("com.apple.TextEdit", "TextEdit", 400);
    a.docs = docs.iter().map(|(t, u)| ((*t).to_string(), *u)).collect();
    a.windows = docs.len() as i64;
    a
}

fn world(docs: &[(&str, bool)]) -> FakeWorld {
    let mut w = FakeWorld::typical(2);
    w.apps.push(textedit(docs));
    w
}

struct Rig {
    s: CtrlService,
    fake: Arc<FakeHelper>,
    clock: Arc<FakeClock>,
}

fn rig(dir: &std::path::Path, w: FakeWorld, wall: u64) -> Rig {
    let fake = Arc::new(FakeHelper::new(w));
    let clock = Arc::new(FakeClock {
        wall: AtomicU64::new(wall),
        mono: AtomicU64::new(0),
    });
    let client = Arc::new(HelperClient::new(Box::new(fake.clone())));
    let s = CtrlService::open_with_zone(
        dir,
        Box::new(ClockHandle(clock.clone())),
        Box::new(IanaZone(Los_Angeles)),
    )
    .with_helper(client);
    Rig { s, fake, clock }
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

fn textedit_running(f: &FakeHelper) -> bool {
    f.world()
        .apps
        .iter()
        .find(|a| a.bundle_id == "com.apple.TextEdit")
        .unwrap()
        .running
}

fn set_docs(f: &FakeHelper, docs: &[(&str, bool)]) {
    let mut w = f.0.lock().unwrap();
    let a = w
        .apps
        .iter_mut()
        .find(|a| a.bundle_id == "com.apple.TextEdit")
        .unwrap();
    *a = textedit(docs);
}

#[test]
fn the_gate_shows_exactly_what_will_happen_and_deny_never_acts() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), world(&[("Notes.txt", false), ("Draft.txt", true)]), T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    let st = &v["state"];
    assert_eq!(st["status"], "needs-confirm", "{v}");
    let c = &st["confirm"];
    assert_eq!(c["action"], "quit an app");
    assert_eq!(c["complete"], true);
    assert_eq!(
        c["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["label"].as_str().unwrap(), i["unsaved"].as_bool().unwrap()))
            .collect::<Vec<_>>(),
        vec![("notes.txt", false), ("draft.txt", true)]
    );
    assert_eq!(
        c["rules"],
        json!(["choose_intent:intent.quit_app", "choose_move:shortcut.cmd+q"])
    );
    assert_eq!(
        r.s.take_confirm_notices().len(),
        1,
        "a notification for the waiting gate"
    );
    assert!(r.fake.world().acted.is_empty());

    let v = confirm(&r.s, &v, "deny");
    assert_eq!(v["state"]["status"], "cancelled", "{v}");
    assert_eq!(v["state"]["confirm"]["answer"], "denied");
    assert!(r.fake.world().acted.is_empty(), "deny never acts");
    assert!(textedit_running(&r.fake));
    assert_eq!(count(&v, "action.requested"), 0);
    // A second answer is refused.
    let again = r.s.call(
        "ctrl:confirm",
        json!({"runId": v["run"]["id"], "gateId": "g0", "answer": "approve"}),
    );
    assert!(again.is_err());
    assert!(r.fake.world().acted.is_empty());
}

#[test]
fn approve_acts_exactly_once_after_the_preview_is_checked_again() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), world(&[("Notes.txt", false)]), T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    let v = confirm(&r.s, &v, "approve");
    assert_eq!(v["state"]["status"], "done", "{v}");
    let w = r.fake.world();
    assert_eq!(w.acted.len(), 1, "one action");
    assert!(!textedit_running(&r.fake));
    assert_eq!(w.frontmost_pid, 100, "the user's app stays frontmost");
    let k = kinds(&v);
    let pos = |name: &str| k.iter().position(|x| x == name).unwrap();
    assert!(pos("confirm.requested") < pos("confirm.resolved"));
    assert!(pos("confirm.resolved") < pos("confirm.reverified"));
    assert!(pos("confirm.reverified") < pos("action.requested"));
    assert_eq!(v["state"]["confirm"]["used"], true);
    assert!(r
        .s
        .call(
            "ctrl:confirm",
            json!({"runId": v["run"]["id"], "gateId": "g0", "answer": "approve"})
        )
        .is_err());
    assert_eq!(
        r.fake.world().acted.len(),
        1,
        "an approval is spent by its one action"
    );
}

#[test]
fn a_changed_world_between_the_answer_and_the_act_asks_again() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), world(&[("Notes.txt", false)]), T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    // While the card waited, the user opened another document.
    set_docs(&r.fake, &[("Notes.txt", false), ("Letter.txt", true)]);
    let v = confirm(&r.s, &v, "approve");
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    assert_eq!(count(&v, "confirm.requested"), 2);
    assert_eq!(v["state"]["confirm"]["gateId"], "g1");
    assert_eq!(v["state"]["confirm"]["items"].as_array().unwrap().len(), 2);
    assert!(r.fake.world().acted.is_empty());
    // Fewer items than approved is still inside the approval.
    set_docs(&r.fake, &[("Notes.txt", false)]);
    let v2 = confirm(&r.s, &v, "approve");
    assert_eq!(v2["state"]["status"], "done", "{v2}");
    assert_eq!(r.fake.world().acted.len(), 1);
}

#[test]
fn a_remembered_approval_is_reused_only_inside_its_scope_and_can_be_revoked() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), world(&[("Notes.txt", false), ("Todo.txt", false)]), T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    let v = confirm(&r.s, &v, "remember");
    assert_eq!(v["state"]["status"], "done", "{v}");
    let tasks = r.s.call("ctrl:tasks", json!({})).unwrap();
    let approvals = tasks[0]["approvals"].as_array().unwrap().clone();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["items"], 2);
    assert_eq!(approvals[0]["unsaved"], 0);
    assert_eq!(approvals[0]["app"], "com.apple.TextEdit");

    // Next time, other titles but fewer windows, nothing unsaved: no question, the standing approval is logged.
    set_docs(&r.fake, &[("Monday.txt", false)]);
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(v["state"]["confirm"]["answer"], "standing");
    assert_eq!(r.s.take_confirm_notices().len(), 1, "only the first run notified");
    assert_eq!(r.fake.world().acted.len(), 2);

    // The scope grew (three windows): ask again.
    set_docs(&r.fake, &[("a", false), ("b", false), ("c", false)]);
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    assert_eq!(r.fake.world().acted.len(), 2);
    confirm(&r.s, &v, "deny");

    // Unsaved work grows the scope too.
    set_docs(&r.fake, &[("a", true)]);
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    confirm(&r.s, &v, "deny");

    // Revoked: the same small scope asks again.
    let id = approvals[0]["id"].as_str().unwrap();
    let task =
        r.s.call("ctrl:revoke-approval", json!({"taskId": t, "approvalId": id}))
            .unwrap();
    assert_eq!(task["approvals"], json!([]));
    set_docs(&r.fake, &[("Monday.txt", false)]);
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    assert_eq!(r.fake.world().acted.len(), 2);
    // Another task's approval never applies.
    let other = create(&r.s, "quit textedit now");
    let v = run_now(&r.s, &other);
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
}

fn log_of(dir: &std::path::Path, v: &Value) -> (EventLog, String) {
    let store = Store::open(dir).unwrap();
    let log = store.run(v["run"]["id"].as_str().unwrap()).unwrap();
    (log, v["run"]["taskId"].as_str().unwrap().to_string())
}

#[test]
fn the_gate_replays_and_a_fork_from_before_the_answer_asks_again() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), world(&[("Secret plan.txt", false)]), T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    let v = confirm(&r.s, &v, "approve");
    assert_eq!(v["state"]["status"], "done");
    let (log, task_id) = log_of(d.path(), &v);
    log.verify().unwrap();
    replay_check(
        &log,
        &Goal {
            task_id: &task_id,
            text: "quit textedit",
        },
    )
    .unwrap();
    let raw = log.to_jsonl().to_lowercase();
    assert!(!raw.contains("secret"), "titles never reach the log");
    // JSON round trip → the same state.
    let back = EventLog::from_jsonl(log.run_id(), &log.to_jsonl()).unwrap();
    assert_eq!(back.state(), log.state());
    // Scrub to just before the answer: the run is waiting again; nothing in a fold acts.
    let resolved = log
        .records()
        .iter()
        .position(|r| matches!(r.event, Event::ConfirmResolved { .. }))
        .unwrap();
    assert!(needs_confirm(&log.state_at(resolved)));
    assert!(!needs_confirm(&log.state()));
    // Fork before the answer: the fork waits for its own answer.
    let fork = log.fork(resolved as u64 - 1, "run-fork");
    fork.verify().unwrap();
    assert!(needs_confirm(&fork.state()));
    // A log that acts on an irreversible plan without the gate doesn't replay.
    let mut forged = EventLog::new("forged");
    for r in log
        .records()
        .iter()
        .take_while(|r| !matches!(r.event, Event::ConfirmRequested { .. }))
    {
        forged.append(r.t_mono_ns, r.t_wall_ms, r.event.clone());
    }
    let act = log
        .records()
        .iter()
        .find(|r| matches!(r.event, Event::ActionRequested { .. }))
        .unwrap();
    let n = forged.records().len() as u64;
    forged.append(act.t_mono_ns, act.t_wall_ms, act.event.clone());
    let err = replay_check(
        &forged,
        &Goal {
            task_id: &task_id,
            text: "quit textedit",
        },
    )
    .unwrap_err();
    assert_eq!(err.seq, n);
}

#[test]
fn a_scheduled_run_waits_for_the_user_then_expires_without_acting() {
    let d = tempfile::tempdir().unwrap();
    let start = IanaZone(Los_Angeles).resolve_local(2026, 10, 19, 8, 0);
    let r = rig(d.path(), world(&[("Notes.txt", false)]), start);
    let t =
        r.s.call(
            "ctrl:create-task",
            json!({"text": "every day quit textedit", "schedule": {"kind": "daily", "hour": 9, "minute": 0}}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
    r.clock.wall.store(start + 3_600_000, Ordering::SeqCst);
    assert_eq!(r.s.tick().unwrap(), vec![t.clone()]);
    let runs = r.s.call("ctrl:runs", json!({"taskId": t})).unwrap();
    assert_eq!(runs[0]["status"], "needs-confirm", "{runs}");
    assert_eq!(
        r.s.take_confirm_notices(),
        vec![(t.clone(), runs[0]["id"].as_str().unwrap().to_string())]
    );
    // The user is away: an hour of ticks later the run still waits and nothing acted.
    for m in 1..=60 {
        r.clock
            .wall
            .store(start + 3_600_000 + m * 60_000, Ordering::SeqCst);
        r.s.tick().unwrap();
    }
    let runs = r.s.call("ctrl:runs", json!({"taskId": t})).unwrap();
    assert_eq!(runs[0]["status"], "needs-confirm");
    assert!(r.fake.world().acted.is_empty());
    // Past the timeout it expires (and says so in ctrl:changed); still nothing acted.
    r.clock
        .wall
        .store(start + 3_600_000 + CONFIRM_TTL_MS, Ordering::SeqCst);
    assert_eq!(r.s.tick().unwrap(), vec![t.clone()]);
    let runs = r.s.call("ctrl:runs", json!({"taskId": t})).unwrap();
    assert_eq!(runs[0]["status"], "expired", "{runs}");
    let v = r.s.call("ctrl:run", json!({"runId": runs[0]["id"]})).unwrap();
    assert_eq!(v["state"]["confirm"]["answer"], "expired");
    assert!(r.fake.world().acted.is_empty());
    assert!(textedit_running(&r.fake));
    // An expired gate can't be approved.
    assert!(r
        .s
        .call(
            "ctrl:confirm",
            json!({"runId": runs[0]["id"], "gateId": "g0", "answer": "approve"})
        )
        .is_err());
}

#[test]
fn a_newer_run_supersedes_a_waiting_gate_and_restart_keeps_waiting() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), world(&[("Notes.txt", false)]), T0);
    let t = create(&r.s, "quit textedit");
    let first = run_now(&r.s, &t);
    drop(r);
    // Relaunch on the same data dir: the gate still waits (nothing acted), and an unanswered gate is found again.
    let r = rig(d.path(), world(&[("Notes.txt", false)]), T0 + 1000);
    r.s.tick().unwrap();
    let v =
        r.s.call("ctrl:run", json!({"runId": first["run"]["id"]}))
            .unwrap();
    assert_eq!(v["state"]["status"], "needs-confirm");
    let second = run_now(&r.s, &t);
    assert_eq!(second["state"]["status"], "needs-confirm");
    let v =
        r.s.call("ctrl:run", json!({"runId": first["run"]["id"]}))
            .unwrap();
    assert_eq!(v["state"]["status"], "expired");
    assert!(r.fake.world().acted.is_empty());
}

#[test]
fn without_an_enumeration_the_preview_falls_back_to_a_count() {
    let d = tempfile::tempdir().unwrap();
    let mut w = world(&[]);
    let te = w
        .apps
        .iter_mut()
        .find(|a| a.bundle_id == "com.apple.TextEdit")
        .unwrap();
    te.windows = 2;
    te.no_preview = true;
    let r = rig(d.path(), w, T0);
    let t = create(&r.s, "quit textedit");
    let v = run_now(&r.s, &t);
    let c = &v["state"]["confirm"];
    assert_eq!(c["complete"], false, "{v}");
    assert_eq!(
        c["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["window 1", "window 2"]
    );
    // Unknown unsaved state errs toward asking: approve still acts once.
    let v = confirm(&r.s, &v, "approve");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(r.fake.world().acted.len(), 1);
}
