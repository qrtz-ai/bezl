//! Slice 3 (ADR 0105): the local scheduler. Advancing a fake clock records exactly one catch-up run, skips
//! paused/deleted/`once`-already-run tasks, and `once` with `at: null` fires ASAP.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::schedule::{IanaZone, LocalZone};
use qrtz_ctrl::service::CtrlService;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

struct FakeClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl FakeClock {
    fn new(wall: u64) -> Self {
        FakeClock {
            wall: AtomicU64::new(wall),
            mono: AtomicU64::new(0),
        }
    }
    fn set(&self, wall: u64) {
        self.wall.store(wall, Ordering::SeqCst);
    }
}
impl Clock for FakeClock {
    fn mono_ns(&self) -> u64 {
        self.mono.fetch_add(1_000_000, Ordering::SeqCst)
    }
    fn wall_ms(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
}

fn ms_la(y: i32, m: u32, d: u32, h: u8, min: u8) -> u64 {
    IanaZone(Los_Angeles).resolve_local(y, m, d, h, min)
}

fn svc(dir: &std::path::Path, clock: FakeClock) -> (CtrlService, std::sync::Arc<FakeClock>) {
    let clock = std::sync::Arc::new(clock);
    let c = clock.clone();
    let service = CtrlService::open_with_zone(dir, Box::new(ClockHandle(c)), Box::new(IanaZone(Los_Angeles)));
    (service, clock)
}

/// Arc wrapper so CtrlService owns the clock trait object while tests still advance it.
struct ClockHandle(std::sync::Arc<FakeClock>);
impl Clock for ClockHandle {
    fn mono_ns(&self) -> u64 {
        self.0.mono_ns()
    }
    fn wall_ms(&self) -> u64 {
        self.0.wall_ms()
    }
}

#[test]
fn tick_records_one_catch_up_after_sleep_and_skips_paused() {
    let d = tempfile::tempdir().unwrap();
    let t0 = ms_la(2026, 6, 1, 8, 0); // Mon 08:00
    let (svc, clock) = svc(d.path(), FakeClock::new(t0));

    let daily = svc
        .call(
            "ctrl:create-task",
            json!({"text": "every day close all my tabs", "schedule": {"kind": "daily", "hour": 9, "minute": 0}}),
        )
        .unwrap();
    let daily_id = daily["id"].as_str().unwrap().to_string();
    let paused = svc
        .call(
            "ctrl:create-task",
            json!({"text": "paused daily", "schedule": {"kind": "daily", "hour": 9, "minute": 0}}),
        )
        .unwrap();
    let paused_id = paused["id"].as_str().unwrap().to_string();
    svc.call("ctrl:set-paused", json!({"id": paused_id, "paused": true}))
        .unwrap();

    // Still before 9: nothing due.
    assert_eq!(svc.tick().unwrap(), Vec::<String>::new());

    // Jump to Thursday 10:00 (missed Mon/Tue/Wed 9:00): exactly one catch-up for the active task.
    clock.set(ms_la(2026, 6, 4, 10, 0));
    let fired = svc.tick().unwrap();
    assert_eq!(fired, vec![daily_id.clone()]);
    let tasks = svc.call("ctrl:tasks", json!(null)).unwrap();
    let active = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == daily_id)
        .unwrap();
    assert_eq!(active["runs"], 1);
    let paused_t = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == paused_id)
        .unwrap();
    assert_eq!(paused_t["runs"], 0);

    // Same instant again: already caught up from `now`, next is Fri 9:00 — no second fire.
    assert_eq!(svc.tick().unwrap(), Vec::<String>::new());

    // Friday 9:00: one more.
    clock.set(ms_la(2026, 6, 5, 9, 0));
    assert_eq!(svc.tick().unwrap(), vec![daily_id.clone()]);
    let tasks = svc.call("ctrl:tasks", json!(null)).unwrap();
    let active = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == daily_id)
        .unwrap();
    assert_eq!(active["runs"], 2);
}

#[test]
fn once_asap_fires_on_first_tick_and_never_again() {
    let d = tempfile::tempdir().unwrap();
    let t0 = ms_la(2026, 6, 1, 12, 0);
    let (svc, clock) = svc(d.path(), FakeClock::new(t0));
    let t = svc
        .call(
            "ctrl:create-task",
            json!({"text": "do this once soon", "schedule": {"kind": "once", "at": null}}),
        )
        .unwrap();
    let id = t["id"].as_str().unwrap().to_string();
    assert_eq!(t["nextRunAt"], t0);

    assert_eq!(svc.tick().unwrap(), vec![id.clone()]);
    assert_eq!(svc.tick().unwrap(), Vec::<String>::new());
    clock.set(t0 + 86_400_000);
    assert_eq!(svc.tick().unwrap(), Vec::<String>::new());
    let tasks = svc.call("ctrl:tasks", json!(null)).unwrap();
    assert_eq!(tasks[0]["runs"], 1);
    assert!(tasks[0]["nextRunAt"].is_null());
}

#[test]
fn once_after_run_now_does_not_fire_from_the_scheduler() {
    let d = tempfile::tempdir().unwrap();
    let at = ms_la(2026, 6, 1, 15, 0);
    let (svc, clock) = svc(d.path(), FakeClock::new(ms_la(2026, 6, 1, 10, 0)));
    let t = svc
        .call(
            "ctrl:create-task",
            json!({"text": "once later", "schedule": {"kind": "once", "at": at}}),
        )
        .unwrap();
    let id = t["id"].as_str().unwrap().to_string();
    svc.call("ctrl:run-now", json!({"id": id})).unwrap();
    clock.set(at + 1);
    assert_eq!(svc.tick().unwrap(), Vec::<String>::new());
}

#[test]
fn deleted_tasks_are_not_scheduled() {
    let d = tempfile::tempdir().unwrap();
    let t0 = ms_la(2026, 6, 1, 8, 0);
    let (svc, clock) = svc(d.path(), FakeClock::new(t0));
    let t = svc
        .call(
            "ctrl:create-task",
            json!({"text": "gone", "schedule": {"kind": "daily", "hour": 9, "minute": 0}}),
        )
        .unwrap();
    let id = t["id"].as_str().unwrap().to_string();
    svc.call("ctrl:delete-task", json!({"id": id})).unwrap();
    clock.set(ms_la(2026, 6, 1, 10, 0));
    assert_eq!(svc.tick().unwrap(), Vec::<String>::new());
}

#[test]
fn run_now_still_writes_pending_intent_only() {
    let d = tempfile::tempdir().unwrap();
    let (svc, _) = svc(d.path(), FakeClock::new(ms_la(2026, 6, 1, 10, 0)));
    let t = svc
        .call(
            "ctrl:create-task",
            json!({"text": "manual", "schedule": {"kind": "once", "at": null}}),
        )
        .unwrap();
    let run = svc.call("ctrl:run-now", json!({"id": t["id"]})).unwrap();
    assert_eq!(run["status"], "pending-intent");
    assert_eq!(run["events"], 2);
}
