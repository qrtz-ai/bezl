//! Slice 2 (ADR 0103): the task store and run logs on disk. Persistence round-trips, a torn last line is
//! dropped, damage is refused (never rewritten), and a run loaded from disk replays, scrubs and forks to the same
//! states as the one that was written.
use qrtz_ctrl::log::{fold, Event, EventLog, TaskStatus};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::service::{CtrlErrorCode, CtrlService};
use qrtz_ctrl::store::{fold_store, Store, StoreError};
use qrtz_ctrl::task::{Schedule, TaskError};
use serde_json::json;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

const T0: u64 = 1_792_300_000_000;
const DAILY9: Schedule = Schedule::Daily { hour: 9, minute: 0 };
const WEEKLY: Schedule = Schedule::Weekly {
    weekday: 1,
    hour: 9,
    minute: 30,
};

struct StepClock(AtomicU64);
impl Clock for StepClock {
    fn mono_ns(&self) -> u64 {
        self.0.load(Ordering::SeqCst) * 1_000_000
    }
    fn wall_ms(&self) -> u64 {
        T0 + self.0.fetch_add(1000, Ordering::SeqCst)
    }
}

fn service(dir: &std::path::Path) -> CtrlService {
    CtrlService::open(dir, Box::new(StepClock(AtomicU64::new(0))))
}

#[test]
fn tasks_persist_and_the_list_is_the_fold_of_the_file() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    let a = s
        .create_task(T0, "every week clean up my downloads folder", WEEKLY)
        .unwrap();
    let b = s
        .create_task(T0 + 1, " every day close all my tabs ", DAILY9)
        .unwrap();
    assert_eq!((a.id.as_str(), b.id.as_str()), ("task-0", "task-1"));
    assert_eq!(b.text, "every day close all my tabs");
    s.set_paused(T0 + 2, &a.id, true).unwrap();
    // Pausing twice writes nothing the second time.
    s.set_paused(T0 + 3, &a.id, true).unwrap();
    assert_eq!(s.records().len(), 3);
    let run = s
        .start_run(
            T0 + 4,
            &b.id,
            &[(
                0,
                Event::TaskStarted {
                    task_id: b.id.clone(),
                },
            )],
        )
        .unwrap();
    assert_eq!(run.run_id(), "run-3");

    let again = Store::open(d.path()).unwrap();
    assert_eq!(again.state(), s.state());
    assert_eq!(fold_store(again.records()), *s.state());
    let ids: Vec<_> = again.tasks().iter().map(|t| t.id.clone()).collect();
    assert_eq!(ids, ["task-1", "task-0"], "newest first");
    assert!(again.task("task-0").unwrap().paused);
    assert_eq!(again.task("task-1").unwrap().runs, ["run-3"]);
    assert_eq!(again.run("run-3").unwrap().records(), run.records());

    let mut again = again;
    again.delete_task(T0 + 5, "task-0").unwrap();
    assert_eq!(Store::open(d.path()).unwrap().tasks().len(), 1);
    assert_eq!(again.delete_task(T0 + 6, "task-0"), Err(StoreError::NotFound));
}

#[test]
fn bad_input_writes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    assert_eq!(
        s.create_task(T0, "   ", DAILY9),
        Err(StoreError::Invalid(TaskError::EmptyText))
    );
    assert_eq!(
        s.create_task(T0, "x", Schedule::Daily { hour: 25, minute: 0 }),
        Err(StoreError::Invalid(TaskError::BadTime))
    );
    assert!(s.records().is_empty());
    assert_eq!(std::fs::read(d.path().join("tasks.jsonl")).ok(), None);
    // Run ids are only ones the store minted: nothing path-like reaches the file system.
    assert_eq!(s.run("../tasks").map(|_| ()), Err(StoreError::NotFound));
    assert_eq!(s.run("run-99").map(|_| ()), Err(StoreError::NotFound));
}

#[test]
fn a_torn_last_line_is_dropped_and_the_store_keeps_working() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    s.create_task(T0, "clean up my downloads", WEEKLY).unwrap();
    let path = d.path().join("tasks.jsonl");
    let whole = std::fs::read(&path).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(br#"{"seq":1,"t_wall_ms":"#)
        .unwrap();
    let mut s = Store::open(d.path()).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        whole,
        "cut back to the last whole record"
    );
    let t = s.create_task(T0 + 1, "close all my tabs", DAILY9).unwrap();
    assert_eq!(t.id, "task-1");
    assert_eq!(Store::open(d.path()).unwrap().tasks().len(), 2);
}

#[test]
fn a_rewritten_record_is_damage_and_is_never_overwritten() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    s.create_task(T0, "clean up my downloads", WEEKLY).unwrap();
    s.create_task(T0 + 1, "close all my tabs", DAILY9).unwrap();
    let path = d.path().join("tasks.jsonl");
    let tampered = std::fs::read_to_string(&path)
        .unwrap()
        .replace("my downloads", "my documents");
    std::fs::write(&path, &tampered).unwrap();
    assert_eq!(
        Store::open(d.path()).map(|_| ()),
        Err(StoreError::Damaged {
            file: "tasks.jsonl".into(),
            line: 0
        })
    );
    let svc = service(d.path());
    let e = svc
        .call(
            "ctrl:create-task",
            json!({"text": "x", "schedule": {"kind": "once", "at": null}}),
        )
        .unwrap_err();
    assert_eq!(e.code, CtrlErrorCode::Internal);
    assert!(e.message.contains("damaged"), "{}", e.message);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        tampered,
        "nothing was written"
    );
}

#[test]
fn a_run_loaded_from_disk_replays_scrubs_and_forks_like_the_one_written() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    let t = s.create_task(T0, "every day close all my tabs", DAILY9).unwrap();
    let mut log = s
        .start_run(
            T0 + 1,
            &t.id,
            &[(
                1,
                Event::TaskStarted {
                    task_id: t.id.clone(),
                },
            )],
        )
        .unwrap();
    for (i, e) in [
        Event::IntentPending {
            task_id: t.id.clone(),
        },
        Event::TaskFinished {
            task_id: t.id.clone(),
            status: TaskStatus::Done,
        },
    ]
    .into_iter()
    .enumerate()
    {
        s.append_run_record(&mut log, 2 + i as u64, T0 + 2 + i as u64, e)
            .unwrap();
    }
    let loaded = Store::open(d.path()).unwrap().run(log.run_id()).unwrap();
    assert_eq!(loaded.to_jsonl(), log.to_jsonl(), "byte-identical");
    loaded.verify().unwrap();
    assert_eq!(loaded.state(), log.state());
    for n in 0..=log.records().len() {
        assert_eq!(loaded.state_at(n), fold(log.events().take(n)), "step {n}");
    }
    assert!(loaded.state_at(2).intent_pending);
    assert!(!loaded.state().intent_pending);
    assert_eq!(loaded.state().task_status, Some(TaskStatus::Done));
    let f: EventLog = loaded.fork(1, "run-fork");
    f.verify().unwrap();
    assert_eq!(f.state(), loaded.state_at(2));
}

#[test]
fn channels_create_list_pause_run_and_scrub() {
    let d = tempfile::tempdir().unwrap();
    let svc = service(d.path());
    // The clock steps 1 s on every call.
    assert_eq!(svc.call("ctrl:tasks", json!(null)).unwrap(), json!([]));
    let t = svc
        .call(
            "ctrl:create-task",
            json!({"text": "every week clean up my downloads folder", "schedule": {"kind": "weekly", "weekday": 0, "hour": 18, "minute": 0}}),
        )
        .unwrap();
    assert_eq!(t["id"], "task-0");
    assert_eq!(t["text"], "every week clean up my downloads folder");
    assert_eq!(
        t["schedule"],
        json!({"kind": "weekly", "weekday": 0, "hour": 18, "minute": 0})
    );
    assert_eq!(
        (
            t["paused"].clone(),
            t["createdAt"].clone(),
            t["runs"].clone(),
            t["lastRunAt"].clone()
        ),
        (json!(false), json!(T0 + 1000), json!(0), json!(null))
    );
    // nextRunAt is the next Sunday 18:00 in the system local zone (ADR 0105).
    assert!(t["nextRunAt"].as_u64().unwrap() > T0 + 1000, "{t}");
    let r = svc.call("ctrl:run-now", json!({"id": "task-0"})).unwrap();
    assert_eq!(
        r,
        json!({"id": "run-1", "taskId": "task-0", "at": T0 + 2000, "status": "planned", "events": 8, "violations": 0})
    );
    let p = svc
        .call("ctrl:set-paused", json!({"id": "task-0", "paused": true}))
        .unwrap();
    assert_eq!(
        (
            p["paused"].clone(),
            p["runs"].clone(),
            p["lastRunAt"].clone(),
            p["nextRunAt"].clone()
        ),
        (json!(true), json!(1), json!(T0 + 2000), json!(null))
    );
    assert_eq!(
        svc.call("ctrl:runs", json!({"taskId": "task-0"})).unwrap(),
        json!([r])
    );

    let v = svc.call("ctrl:run", json!({"runId": "run-1", "at": 1})).unwrap();
    assert_eq!(v["at"], 1);
    // task.started, intent.pending, obs.apps, 4 decisions (intent, app, folder, move), plan.produced (ADR 0106).
    assert_eq!(v["events"].as_array().unwrap().len(), 8);
    assert_eq!(v["events"][1]["kind"], "intent.pending");
    assert_eq!(v["events"][3]["kind"], "decision.made");
    assert_eq!(v["events"][3]["node"], "choose_intent");
    assert_eq!(v["state"]["status"], "running", "after task.started only");
    let at2 = svc.call("ctrl:run", json!({"runId": "run-1", "at": 2})).unwrap();
    assert_eq!(at2["state"]["status"], "pending-intent");
    let full = svc.call("ctrl:run", json!({"runId": "run-1"})).unwrap();
    assert_eq!(full["at"], 8);
    assert_eq!(full["state"]["status"], "planned");
    assert_eq!(
        svc.call("ctrl:run", json!({"runId": "run-1", "at": 99})).unwrap()["at"],
        8,
        "clamped"
    );

    // A fresh service over the same folder sees the same thing.
    let svc2 = service(d.path());
    assert_eq!(
        svc2.call("ctrl:runs", json!({"taskId": "task-0"})).unwrap(),
        json!([r])
    );
    assert_eq!(
        svc2.call("ctrl:delete-task", json!({"id": "task-0"})).unwrap(),
        json!({"ok": true})
    );
    assert_eq!(svc2.call("ctrl:tasks", json!(null)).unwrap(), json!([]));
    assert_eq!(
        svc2.call("ctrl:run-now", json!({"id": "task-0"}))
            .unwrap_err()
            .code,
        CtrlErrorCode::NotFound
    );
    assert_eq!(
        svc2.call(
            "ctrl:create-task",
            json!({"text": "x", "schedule": {"kind": "hourly"}})
        )
        .unwrap_err()
        .code,
        CtrlErrorCode::Validation
    );
}
