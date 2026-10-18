//! ADR 0122: "every friday empty the trash" end to end on Linux, through the `ctrl:*` channels against a temp
//! "Trash". Emptying is irreversible, so every run stops at the confirm gate with a `file` preview (hashed names in
//! the log, readable names in memory only). Deny deletes nothing, a Trash that grew before the act asks again,
//! approve deletes exactly the listed items (never what was trashed in the last 10 minutes, never the Finder's own
//! hidden files), a remembered approval is bounded by its count, a scheduled run waits and then expires, nothing
//! touches the screen, there is no undo, and a log that deletes without an approved gate doesn't replay.
use chrono_tz::America::Los_Angeles;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::files::{plan_empty_trash, FileRoots, Planned};
use qrtz_ctrl::fs_exec::{FsExecutor, MemStore};
use qrtz_ctrl::gate::CONFIRM_TTL_MS;
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::ladder::{FsOp, Route};
use qrtz_ctrl::log::{Event, EventLog};
use qrtz_ctrl::runner::{Clock, Executor};
use qrtz_ctrl::schedule::{IanaZone, LocalZone};
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const T0: u64 = 1_790_000_000_000;
const DAY: u64 = 24 * 3600 * 1000;
const TASK: &str = "every friday empty the trash";

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

fn at(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

/// A file modified `age_ms` before `now`.
fn put(dir: &Path, name: &str, now: u64, age_ms: u64) -> PathBuf {
    let p = dir.join(name);
    let f = File::create(&p).unwrap();
    f.set_modified(at(now - age_ms)).unwrap();
    p
}

struct Rig {
    s: CtrlService,
    fake: Arc<FakeHelper>,
    clock: Arc<FakeClock>,
    roots: FileRoots,
    d: tempfile::TempDir,
}

fn rig_at(wall: u64) -> Rig {
    let d = tempfile::tempdir().unwrap();
    let roots = FileRoots {
        downloads: d.path().join("Downloads"),
        trash: d.path().join("Trash"),
        desktop: d.path().join("Desktop"),
    };
    fs::create_dir_all(&roots.downloads).unwrap();
    fs::create_dir_all(&roots.trash).unwrap();
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(1)));
    let clock = Arc::new(FakeClock {
        wall: AtomicU64::new(wall),
        mono: AtomicU64::new(0),
    });
    let s = CtrlService::open_with_zone(
        d.path().join("store"),
        Box::new(ClockHandle(clock.clone())),
        Box::new(IanaZone(Los_Angeles)),
    )
    .with_helper(Arc::new(HelperClient::new(Box::new(fake.clone()))))
    .with_files(roots.clone());
    Rig {
        s,
        fake,
        clock,
        roots,
        d,
    }
}

fn rig() -> Rig {
    rig_at(T0)
}

/// Two old items, one just trashed, and the Finder's own `.DS_Store`.
fn typical(r: &Rig) {
    let t = &r.roots.trash;
    put(t, "old.dmg", T0, 3 * DAY);
    let folder = t.join("Old Folder");
    fs::create_dir(&folder).unwrap();
    put(&folder, "secret notes.txt", T0, 9 * DAY);
    File::open(&folder)
        .unwrap()
        .set_modified(at(T0 - 5 * DAY))
        .unwrap();
    put(t, "fresh.png", T0, 60 * 1000);
    put(t, ".DS_Store", T0, 9 * DAY);
}

fn listing(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = walk(dir)
        .into_iter()
        .map(|e| e.strip_prefix(dir).unwrap().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() && !p.is_symlink() {
            v.push(p.clone());
            v.extend(walk(&p));
        } else {
            v.push(p);
        }
    }
    v
}

fn create(s: &CtrlService, text: &str) -> String {
    s.call(
        "ctrl:create-task",
        json!({"text": text, "schedule": {"kind": "weekly", "weekday": 5, "hour": 18, "minute": 0}}),
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

fn count(v: &Value, kind: &str) -> usize {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind)
        .count()
}

fn labels(v: &Value) -> Vec<String> {
    v["state"]["confirm"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            assert_eq!(i["kind"], "file");
            i["label"].as_str().unwrap().to_string()
        })
        .collect()
}

fn untouched(r: &Rig) {
    let w = r.fake.world();
    let t = FakeWorld::typical(0);
    assert_eq!(w.frontmost_pid, t.frontmost_pid, "frontmost app changed");
    assert_eq!(w.cursor, t.cursor, "cursor moved");
    assert_eq!(w.focused, t.focused, "focus changed");
    assert!(w.acted.is_empty(), "no helper act: the Finder is never driven");
}

/// Nothing persisted holds a name or a path.
fn no_names_on_disk(r: &Rig, names: &[&str]) {
    for f in walk(&r.d.path().join("store")) {
        if f.is_file() {
            let text = fs::read_to_string(&f).unwrap_or_default().to_lowercase();
            for n in names {
                assert!(!text.contains(&n.to_lowercase()), "{n} reached {}", f.display());
            }
            assert!(
                !text.contains(&*r.roots.trash.to_string_lossy().to_lowercase()),
                "a path reached the log"
            );
        }
    }
}

fn log_of(r: &Rig, v: &Value) -> EventLog {
    Store::open(r.d.path().join("store"))
        .unwrap()
        .run(v["run"]["id"].as_str().unwrap())
        .unwrap()
}

#[test]
fn emptying_the_trash_waits_at_the_gate_deny_deletes_nothing_a_grown_trash_asks_again() {
    let r = rig();
    typical(&r);
    let t = create(&r.s, TASK);
    let v = run_now(&r.s, &t);
    let st = &v["state"];
    assert_eq!(st["status"], "needs-confirm", "{v}");
    assert_eq!(st["confirm"]["action"], "empty the trash");
    assert_eq!(
        labels(&v),
        vec!["old folder", "old.dmg"],
        "only items trashed over 10 minutes ago; never the finder's own files"
    );
    assert_eq!(count(&v, "action.requested"), 0, "nothing acts before the answer");
    assert_eq!(r.s.take_confirm_notices().len(), 1);
    no_names_on_disk(&r, &["old.dmg", "Old Folder", "secret", "fresh.png"]);
    let before = listing(&r.roots.trash);

    // Deny: nothing is deleted, the run doesn't count as done.
    let denied = confirm(&r.s, &v, "deny");
    assert_ne!(denied["state"]["status"], "done", "{denied}");
    assert_eq!(listing(&r.roots.trash), before);

    // A new run, and something else lands in the Trash before the answer: the re-check asks again.
    let v = run_now(&r.s, &t);
    put(&r.roots.trash, "grown.zip", T0, 2 * DAY);
    let grown = confirm(&r.s, &v, "approve");
    assert_eq!(grown["state"]["status"], "needs-confirm", "{grown}");
    assert_eq!(labels(&grown), vec!["old folder", "grown.zip", "old.dmg"]);
    assert_eq!(count(&grown, "action.requested"), 0);
    assert!(
        r.roots.trash.join("old.dmg").exists(),
        "a grown trash deleted nothing"
    );

    // Approve the fresh card: exactly those three are gone, the fresh file and .DS_Store stay.
    let done = confirm(&r.s, &grown, "approve");
    assert_eq!(done["state"]["status"], "done", "{done}");
    assert_eq!(listing(&r.roots.trash), vec![".DS_Store", "fresh.png"]);
    assert_eq!(count(&done, "action.requested"), 1, "one batch");
    assert_eq!(
        done["state"]["undo"],
        Value::Null,
        "emptying the trash has no put back"
    );
    untouched(&r);
    no_names_on_disk(&r, &["old.dmg", "Old Folder", "secret", "grown.zip"]);

    // The log replays; the same log acting before any gate doesn't.
    let log = log_of(&r, &done);
    log.verify().unwrap();
    let goal = Goal {
        task_id: &t,
        text: TASK,
    };
    replay_check(&log, &goal).unwrap();
    let mut forged = EventLog::new("forged");
    for rec in log
        .records()
        .iter()
        .take_while(|rec| !matches!(rec.event, Event::ConfirmRequested { .. }))
    {
        forged.append(rec.t_mono_ns, rec.t_wall_ms, rec.event.clone());
    }
    let act = log
        .records()
        .iter()
        .find(|rec| matches!(rec.event, Event::ActionRequested { .. }))
        .unwrap();
    let n = forged.records().len() as u64;
    forged.append(act.t_mono_ns, act.t_wall_ms, act.event.clone());
    assert_eq!(replay_check(&forged, &goal).unwrap_err().seq, n);

    // An empty Trash (only what ctrl leaves alone): done, no card.
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(count(&v, "confirm.requested"), 0);
    assert_eq!(listing(&r.roots.trash), vec![".DS_Store", "fresh.png"]);
}

#[test]
fn empty_my_trash_is_the_same_task_and_a_remembered_approval_is_bounded_by_its_count() {
    let r = rig();
    put(&r.roots.trash, "a.txt", T0, DAY);
    put(&r.roots.trash, "b.txt", T0, DAY);
    let t = create(&r.s, "empty my trash");
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    assert_eq!(v["state"]["confirm"]["move"], "empty the trash");
    let v = confirm(&r.s, &v, "remember");
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert!(listing(&r.roots.trash).is_empty());
    let tasks = r.s.call("ctrl:tasks", json!({})).unwrap();
    let approvals = tasks[0]["approvals"].as_array().unwrap().clone();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["items"], 2);
    assert_eq!(approvals[0]["app"], Value::Null);
    r.s.take_confirm_notices();

    // Next week, other names, no more items: it runs unattended, the standing approval is logged.
    put(&r.roots.trash, "c.txt", T0, DAY);
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "done", "{v}");
    assert_eq!(v["state"]["confirm"]["answer"], "standing");
    assert!(listing(&r.roots.trash).is_empty());
    assert!(r.s.take_confirm_notices().is_empty(), "no question asked");
    replay_check(
        &log_of(&r, &v),
        &Goal {
            task_id: &t,
            text: "empty my trash",
        },
    )
    .unwrap();

    // A bigger Trash than was approved: ask, nothing deleted.
    for n in ["d", "e", "f"] {
        put(&r.roots.trash, &format!("{n}.txt"), T0, DAY);
    }
    let v = run_now(&r.s, &t);
    assert_eq!(v["state"]["status"], "needs-confirm", "{v}");
    assert_eq!(listing(&r.roots.trash).len(), 3);
    untouched(&r);
}

#[test]
fn a_scheduled_friday_run_waits_at_the_gate_then_expires_and_deletes_nothing() {
    // Thursday 2026-10-22 08:00 PT; the task runs Fridays 18:00.
    let start = IanaZone(Los_Angeles).resolve_local(2026, 10, 22, 8, 0);
    let r = rig_at(start);
    put(&r.roots.trash, "old.dmg", start, 3 * DAY);
    let t = create(&r.s, TASK);
    r.clock.wall.store(start + 10 * 3_600_000, Ordering::SeqCst);
    assert!(r.s.tick().unwrap().is_empty(), "not friday 18:00 yet");
    r.clock.wall.store(start + 35 * 3_600_000, Ordering::SeqCst);
    assert_eq!(r.s.tick().unwrap(), vec![t.clone()]);
    let runs = r.s.call("ctrl:runs", json!({"taskId": t})).unwrap();
    assert_eq!(runs[0]["status"], "needs-confirm", "{runs}");
    assert_eq!(r.s.take_confirm_notices().len(), 1);
    // Away for the evening: still waiting, nothing deleted.
    for h in 1..=6 {
        r.clock.wall.store(start + (35 + h) * 3_600_000, Ordering::SeqCst);
        r.s.tick().unwrap();
    }
    assert!(r.roots.trash.join("old.dmg").exists());
    r.clock
        .wall
        .store(start + 35 * 3_600_000 + CONFIRM_TTL_MS, Ordering::SeqCst);
    assert_eq!(r.s.tick().unwrap(), vec![t.clone()]);
    let runs = r.s.call("ctrl:runs", json!({"taskId": t})).unwrap();
    assert_eq!(runs[0]["status"], "expired", "{runs}");
    assert!(
        r.roots.trash.join("old.dmg").exists(),
        "an expired gate never acts"
    );
    assert!(r
        .s
        .call(
            "ctrl:confirm",
            json!({"runId": runs[0]["id"], "gateId": "g0", "answer": "approve"})
        )
        .is_err());
    untouched(&r);
}

#[test]
fn the_planner_skips_just_trashed_and_the_executor_only_deletes_inside_the_trash() {
    let d = tempfile::tempdir().unwrap();
    let roots = FileRoots {
        downloads: d.path().join("Downloads"),
        trash: d.path().join("Trash"),
        desktop: d.path().join("Desktop"),
    };
    // No Trash folder yet: nothing to do.
    assert!(plan_empty_trash(&roots, SystemTime::now())
        .unwrap()
        .ops
        .is_empty());
    fs::create_dir_all(&roots.trash).unwrap();
    fs::create_dir_all(&roots.downloads).unwrap();
    // Edited a month ago but trashed (renamed in) just now: its inode change time is fresh, so it stays.
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    let elsewhere = put(&roots.downloads, "report.pdf", now_ms, 30 * DAY);
    fs::rename(&elsewhere, roots.trash.join("report.pdf")).unwrap();
    let p = plan_empty_trash(&roots, SystemTime::now()).unwrap();
    assert!(p.ops.is_empty(), "just trashed: {p:?}");
    // Eleven minutes later it's due.
    let later = SystemTime::now() + Duration::from_secs(11 * 60);
    let p = plan_empty_trash(&roots, later).unwrap();
    assert_eq!(
        p.ops,
        vec![Planned::Delete {
            src: roots.trash.join("report.pdf")
        }]
    );
    assert!(p.deletes());

    // The executor refuses anything not directly in the Trash, never follows a symlink, and has no undo.
    let keep = put(&roots.downloads, "keep.txt", now_ms, DAY);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&keep, roots.trash.join("link")).unwrap();
    let mut store = MemStore::default();
    let outside = store.put("f1", &keep);
    let link = store.put("f2", roots.trash.join("link"));
    let mut ex = FsExecutor {
        store,
        trash_dir: roots.trash.clone(),
    };
    let err = ex
        .execute(&Route::Files(FsOp::Delete { src: outside.clone() }))
        .unwrap_err();
    assert_eq!(err, "fs.outside_trash");
    assert!(keep.exists());
    #[cfg(unix)]
    {
        ex.execute(&Route::Files(FsOp::Delete { src: link.clone() }))
            .unwrap();
        assert!(!roots.trash.join("link").exists());
        assert!(keep.exists(), "a symlink is removed itself, never followed");
    }
    assert_eq!(
        ex.undo(&FsOp::Delete { src: link }).unwrap_err(),
        "fs.irreversible"
    );
}
