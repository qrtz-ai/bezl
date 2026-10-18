//! The guarded runner and the event log (ADR 0100): snapshots around every action, violations recorded and
//! quarantined, and a log that replays, scrubs and forks deterministically.
use qrtz_ctrl::fs_exec::{FsExecutor, MemStore};
use qrtz_ctrl::guard::{Field, Point, UiSnapshot};
use qrtz_ctrl::ladder::*;
use qrtz_ctrl::log::{ActionStatus, Event, EventLog, LogError, Phase};
use qrtz_ctrl::runner::{Clock, Executor, RunOutcome, Runner, UiProbe};
use std::cell::{Cell, RefCell};

const SAFARI: &str = "com.apple.Safari";

/// A fake session: what the user owns lives here, and a misbehaving executor can change it.
#[derive(Default)]
struct Session {
    ui: RefCell<UiSnapshot>,
}

impl UiProbe for Session {
    fn snapshot(&self) -> UiSnapshot {
        self.ui.borrow().clone()
    }
}

struct TickClock(Cell<u64>);
impl Clock for TickClock {
    fn mono_ns(&self) -> u64 {
        let t = self.0.get() + 7_000_000; // 7 ms per read
        self.0.set(t);
        t
    }
    fn wall_ms(&self) -> u64 {
        1_792_300_000_000 + self.0.get() / 1_000_000
    }
}

fn user_desk() -> UiSnapshot {
    UiSnapshot {
        frontmost_pid: Some(501),
        cursor: Some(Point { x: 812.0, y: 433.5 }),
        focused_element: Some("e:9f3c".into()),
        key_window: Some(4242),
        hid_idle_ms: Some(60_000),
    }
}

/// Activates the target app on every call, like a script with `activate` in it would.
struct Thief<'a> {
    session: &'a Session,
    restores: u32,
}
impl Executor for Thief<'_> {
    fn execute(&mut self, _route: &Route) -> Result<(), String> {
        let mut ui = self.session.ui.borrow_mut();
        ui.frontmost_pid = Some(733);
        ui.focused_element = Some("e:0000".into());
        ui.key_window = Some(9);
        Ok(())
    }
    fn restore(&mut self, before: &UiSnapshot, fields: &[Field]) -> bool {
        self.restores += 1;
        assert!(fields.contains(&Field::FrontmostApp));
        *self.session.ui.borrow_mut() = before.clone();
        true
    }
}

fn env() -> Env {
    Env {
        frontmost_app: Some("com.microsoft.VSCode".into()),
        user_idle_ms: Some(60_000),
        files: Some(Consent::Granted),
        accessibility: Some(Consent::Granted),
        post_event: Some(Consent::Granted),
        automation: [(SAFARI.to_string(), Consent::Granted)].into(),
        ..Default::default()
    }
}

#[test]
fn downloads_cleanup_moves_files_and_leaves_the_user_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let downloads = tmp.path().join("Downloads");
    let archive = downloads.join("old");
    std::fs::create_dir_all(&archive).unwrap();
    for n in ["a.dmg", "b.pdf", "c.zip"] {
        std::fs::write(downloads.join(n), n).unwrap();
    }
    let mut store = MemStore::default();
    let dir = store.put("c:archive", &archive);
    let files: Vec<_> = ["a.dmg", "b.pdf", "c.zip"]
        .iter()
        .enumerate()
        .map(|(i, n)| store.put(&format!("c:{i}"), downloads.join(n)))
        .collect();
    let mut exec = FsExecutor {
        store,
        trash_dir: tmp.path().join(".Trash"),
    };
    let session = Session {
        ui: RefCell::new(user_desk()),
    };
    let clock = TickClock(Cell::new(0));
    let mut log = EventLog::new("run-1");
    log.append(
        clock.mono_ns(),
        clock.wall_ms(),
        Event::TaskStarted {
            task_id: "t-downloads".into(),
        },
    );

    let mut runner = Runner {
        log: &mut log,
        probe: &session,
        exec: &mut exec,
        clock: &clock,
    };
    for (i, f) in files.iter().enumerate() {
        let route = if i == 2 {
            Route::Files(FsOp::Trash { src: f.clone() })
        } else {
            Route::Files(FsOp::MoveInto {
                src: f.clone(),
                dst_dir: dir.clone(),
            })
        };
        // The planner also offered a Finder AX route; the ladder must pick the file route.
        let routes = vec![
            Route::Ax {
                app: "com.apple.finder".into(),
                role: "AXRow".into(),
                element: f.clone(),
                op: AxOp::Perform {
                    action: "AXPress".into(),
                },
            },
            route,
        ];
        assert_eq!(
            runner.run(&format!("a{i}"), &routes, &env()),
            RunOutcome::Done { clean: true }
        );
    }
    assert!(archive.join("a.dmg").is_file() && archive.join("b.pdf").is_file());
    assert!(tmp.path().join(".Trash/c.zip").is_file());
    assert!(!downloads.join("a.dmg").exists());

    // A second move onto an existing name fails without overwriting.
    std::fs::write(downloads.join("a.dmg"), "new").unwrap();
    let again = exec_again(&mut log, &session, &clock, tmp.path(), &downloads, &archive);
    assert!(matches!(again, RunOutcome::Failed { ref err, clean: true } if err == "fs.exists"));
    assert_eq!(std::fs::read_to_string(archive.join("a.dmg")).unwrap(), "a.dmg");

    let s = log.state();
    assert_eq!(s.violations, 0);
    assert!(s.quarantined.is_empty());
    assert_eq!(s.actions["a0"], ActionStatus::Done);
    assert_eq!(s.actions["a3"], ActionStatus::Failed);
    log.verify().unwrap();
    // Every action has a before and after snapshot, and the log carries refs, never paths.
    let snaps = log
        .events()
        .filter(|e| matches!(e, Event::GuardSnapshot { .. }))
        .count();
    assert_eq!(snaps, 8);
    let jsonl = log.to_jsonl();
    assert!(
        !jsonl.contains("Downloads") && !jsonl.contains(".dmg"),
        "paths leaked into the log"
    );
}

fn exec_again(
    log: &mut EventLog,
    session: &Session,
    clock: &TickClock,
    root: &std::path::Path,
    downloads: &std::path::Path,
    archive: &std::path::Path,
) -> RunOutcome {
    let mut store = MemStore::default();
    let src = store.put("c:again", downloads.join("a.dmg"));
    let dir = store.put("c:archive", archive);
    let mut exec = FsExecutor {
        store,
        trash_dir: root.join(".Trash"),
    };
    let mut runner = Runner {
        log,
        probe: session,
        exec: &mut exec,
        clock,
    };
    runner.run(
        "a3",
        &[Route::Files(FsOp::MoveInto { src, dst_dir: dir })],
        &env(),
    )
}

#[test]
fn a_violation_is_recorded_restored_and_quarantines_the_rung() {
    let session = Session {
        ui: RefCell::new(user_desk()),
    };
    let clock = TickClock(Cell::new(0));
    let mut log = EventLog::new("run-2");
    let mut thief = Thief {
        session: &session,
        restores: 0,
    };
    let routes = vec![
        Route::Scripting {
            app: SAFARI.into(),
            verb: ScriptVerb::CloseAllTabs,
        },
        Route::Ax {
            app: SAFARI.into(),
            role: "AXButton".into(),
            element: ContentRef("c:x".into()),
            op: AxOp::Perform {
                action: "AXPress".into(),
            },
        },
    ];
    {
        let mut runner = Runner {
            log: &mut log,
            probe: &session,
            exec: &mut thief,
            clock: &clock,
        };
        assert_eq!(
            runner.run("a0", &routes, &env()),
            RunOutcome::Done { clean: false }
        );
    }
    assert_eq!(thief.restores, 1);
    assert_eq!(
        *session.ui.borrow(),
        user_desk(),
        "restore put the user's app back"
    );
    let s = log.state();
    assert_eq!(s.violations, 3); // frontmost, focused element, key window
    assert!(s.quarantined.contains(&(SAFARI.to_string(), Rung::Scripting)));
    let restored = log
        .events()
        .any(|e| matches!(e, Event::GuardRestored { ok: true, .. }));
    assert!(restored);

    // The next action for Safari skips the quarantined scripting rung and drops to AX.
    {
        let mut runner = Runner {
            log: &mut log,
            probe: &session,
            exec: &mut thief,
            clock: &clock,
        };
        runner.run("a1", &routes, &env());
    }
    let decided: Vec<_> = log
        .events()
        .filter_map(|e| match e {
            Event::ActionDecided {
                action_id, decision, ..
            } if action_id == "a1" => Some(decision.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        decided,
        vec![Decision::Execute {
            index: 1,
            rung: Rung::AxAction
        }]
    );
    // ... which also violated, so both rungs are now quarantined and a third try refuses.
    {
        let mut runner = Runner {
            log: &mut log,
            probe: &session,
            exec: &mut thief,
            clock: &clock,
        };
        assert_eq!(runner.run("a2", &routes, &env()), RunOutcome::Refused);
    }
    assert_eq!(thief.restores, 2);
}

#[test]
fn a_change_while_the_user_was_touching_the_mouse_is_theirs() {
    struct UserMoves<'a>(&'a Session);
    impl Executor for UserMoves<'_> {
        fn execute(&mut self, _r: &Route) -> Result<(), String> {
            let mut ui = self.0.ui.borrow_mut();
            ui.cursor = Some(Point { x: 1.0, y: 1.0 });
            ui.hid_idle_ms = Some(2); // physical input during the action
            Ok(())
        }
    }
    let session = Session {
        ui: RefCell::new(user_desk()),
    };
    let clock = TickClock(Cell::new(0));
    let mut log = EventLog::new("run-3");
    let mut ex = UserMoves(&session);
    let mut runner = Runner {
        log: &mut log,
        probe: &session,
        exec: &mut ex,
        clock: &clock,
    };
    let routes = [Route::Workspace(WorkspaceOp::LaunchHidden {
        app: "com.apple.TextEdit".into(),
    })];
    assert_eq!(
        runner.run("a0", &routes, &env()),
        RunOutcome::Done { clean: true }
    );
    let s = log.state();
    assert_eq!((s.violations, s.inconclusive), (0, 1));
}

#[test]
fn refused_and_asked_actions_never_snapshot_or_execute() {
    struct Panics;
    impl Executor for Panics {
        fn execute(&mut self, _r: &Route) -> Result<(), String> {
            panic!("must not execute")
        }
    }
    let session = Session {
        ui: RefCell::new(user_desk()),
    };
    let clock = TickClock(Cell::new(0));
    let mut log = EventLog::new("run-4");
    let mut ex = Panics;
    let mut runner = Runner {
        log: &mut log,
        probe: &session,
        exec: &mut ex,
        clock: &clock,
    };
    assert_eq!(
        runner.run("a0", &[Route::Click { app: SAFARI.into() }], &env()),
        RunOutcome::Refused
    );
    let mut no_ax = env();
    no_ax.accessibility = Some(Consent::NotAsked);
    let ax = Route::Ax {
        app: SAFARI.into(),
        role: "AXButton".into(),
        element: ContentRef("c".into()),
        op: AxOp::Perform {
            action: "AXPress".into(),
        },
    };
    assert_eq!(runner.run("a1", &[ax], &no_ax), RunOutcome::Asked);
    assert!(!log.events().any(|e| matches!(e, Event::GuardSnapshot { .. })));
    assert_eq!(log.state().actions["a0"], ActionStatus::Refused);
    assert_eq!(log.state().actions["a1"], ActionStatus::Asked);
}

fn sample_log() -> EventLog {
    let session = Session {
        ui: RefCell::new(user_desk()),
    };
    let clock = TickClock(Cell::new(0));
    let mut log = EventLog::new("run-5");
    let mut thief = Thief {
        session: &session,
        restores: 0,
    };
    let mut runner = Runner {
        log: &mut log,
        probe: &session,
        exec: &mut thief,
        clock: &clock,
    };
    let routes = [Route::Scripting {
        app: SAFARI.into(),
        verb: ScriptVerb::CloseAllTabs,
    }];
    runner.run("a0", &routes, &env());
    runner.run("a1", &routes, &env());
    log
}

#[test]
fn the_log_replays_byte_identically_and_detects_tampering() {
    let log = sample_log();
    log.verify().unwrap();
    let jsonl = log.to_jsonl();
    let back = EventLog::from_jsonl("run-5", &jsonl).unwrap();
    back.verify().unwrap();
    assert_eq!(back.to_jsonl(), jsonl);
    assert_eq!(
        serde_json::to_string(&back.state()).unwrap(),
        serde_json::to_string(&log.state()).unwrap()
    );

    let tampered = jsonl.replacen("\"frontmost_pid\":733", "\"frontmost_pid\":501", 1);
    assert_ne!(tampered, jsonl);
    let bad = EventLog::from_jsonl("run-5", &tampered).unwrap();
    assert!(matches!(bad.verify(), Err(LogError::BrokenChain { .. })));
}

#[test]
fn scrub_and_fork() {
    let log = sample_log();
    // Scrubbing: state after the first action's guard result has the quarantine, before it doesn't.
    let at = log
        .records()
        .iter()
        .position(|r| matches!(r.event, Event::GuardResult { .. }))
        .unwrap();
    assert!(log.state_at(at).quarantined.is_empty());
    assert_eq!(log.state_at(at + 1).quarantined.len(), 1);
    // Fork just before the guard result: the branch hasn't seen the violation.
    let fork = log.fork(at as u64 - 1, "run-5b");
    fork.verify().unwrap();
    assert_eq!(fork.records()[0].parent_seq, Some(at as u64 - 1));
    assert_eq!(fork.records().len(), at);
    assert!(fork.state().quarantined.is_empty());
    assert!(fork.events().any(|e| matches!(
        e,
        Event::GuardSnapshot {
            phase: Phase::After,
            ..
        }
    )));
}
