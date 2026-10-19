//! Slice 5 (ADR 0108) end to end on Linux: the `ctrl:*` channels drive a run through the decision layers and the
//! executor, over the real ctrl helper protocol (JSON lines) to the in-memory `FakeHelper`. Every run must leave
//! the user's frontmost app, cursor and focused element as they were, and the persisted run must replay.
use qrtz_ctrl::ax::AxNodeWire;
use qrtz_ctrl::decide::{replay_check, Goal};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient, HelperMsg, HostMsg};
use qrtz_ctrl::ladder::{Route, ScriptVerb};
use qrtz_ctrl::log::Event;
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::service::CtrlService;
use qrtz_ctrl::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const T0: u64 = 1_790_000_000_000;

struct Tick(AtomicU64);
impl Clock for Tick {
    fn mono_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) * 1000
    }
    fn wall_ms(&self) -> u64 {
        T0
    }
}

fn svc(dir: &std::path::Path, world: FakeWorld) -> (CtrlService, Arc<FakeHelper>) {
    let fake = Arc::new(FakeHelper::new(world));
    let client = Arc::new(HelperClient::new(Box::new(fake.clone())));
    let s = CtrlService::open(dir, Box::new(Tick(AtomicU64::new(0)))).with_helper(client);
    (s, fake)
}

fn run(s: &CtrlService, text: &str) -> Value {
    let id = s
        .call(
            "ctrl:create-task",
            json!({"text": text, "schedule": {"kind": "once", "at": null}}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = s.call("ctrl:run-now", json!({"id": id})).unwrap();
    s.call("ctrl:run", json!({"runId": r["id"]})).unwrap()
}

/// What the user owns is exactly as it was (ADR 0100).
fn untouched(w: &FakeWorld) {
    let t = FakeWorld::typical(0);
    assert_eq!(w.frontmost_pid, t.frontmost_pid, "frontmost app changed");
    assert_eq!(w.cursor, t.cursor, "cursor moved");
    assert_eq!(w.focused, t.focused, "focus changed");
}

fn kinds(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

/// The persisted run: replays to the same questions and holds no task text or window titles (bundle ids are ids).
fn replays(dir: &std::path::Path, v: &Value, text: &str, banned: &[&str]) {
    let store = Store::open(dir).unwrap();
    let run_id = v["run"]["id"].as_str().unwrap();
    let log = store.run(run_id).unwrap();
    log.verify().unwrap();
    let task_id = v["run"]["taskId"].as_str().unwrap();
    replay_check(&log, &Goal { task_id, text }).unwrap();
    let raw = log.to_jsonl().to_lowercase();
    for w in banned {
        assert!(!raw.contains(w), "the log holds {w:?}");
    }
    // Scrub and fork still fold.
    let n = log.records().len();
    assert_eq!(log.state_at(n), log.state());
    assert_eq!(log.fork(n as u64 - 1, "fork").state(), log.state());
}

#[test]
fn close_all_tabs_runs_by_script_in_the_background_and_verifies() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = svc(d.path(), FakeWorld::typical(5));
    let text = "every day close all my tabs";
    let v = run(&s, text);
    let st = &v["state"];
    assert_eq!(st["status"], "done", "{v}");
    let w = fake.world();
    let safari = w.apps.iter().find(|a| a.bundle_id == "com.apple.Safari").unwrap();
    assert_eq!(safari.tabs, 0);
    assert_eq!(
        w.acted,
        vec![(
            "a0".to_string(),
            Route::Scripting {
                app: "com.apple.Safari".into(),
                verb: ScriptVerb::CloseSavedTabs
            }
        )]
    );
    untouched(&w);
    assert_eq!(st["violations"], 0);
    assert_eq!(st["verified"], json!([{"actionId": "a0", "outcome": "passed"}]));
    // The real default browser came from the helper's catalog, and its front window was observed first.
    assert_eq!(st["decisions"][1]["options"][0]["id"], "app.com.apple.Safari");
    let k = kinds(&v);
    for want in [
        "obs.apps",
        "obs.snapshot",
        "plan.produced",
        "action.requested",
        "guard.snapshot",
        "action.executed",
        "guard.result",
        "verify.result",
        "task.finished",
    ] {
        assert!(k.iter().any(|x| x == want), "missing {want}: {k:?}");
    }
    replays(d.path(), &v, text, &["close all", "my tabs"]);
}

#[test]
fn open_a_site_opens_the_address_in_the_running_default_browser_without_activating_it() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = svc(d.path(), FakeWorld::typical(2));
    let text = "open youtube";
    let v = run(&s, text);
    assert_eq!(v["state"]["status"], "done", "{v}");
    let w = fake.world();
    assert_eq!(w.opened_urls, vec!["https://youtube.com".to_string()]);
    untouched(&w);
    // The URL went to the helper for that call only; the log keeps a pointer into the task text.
    replays(d.path(), &v, text, &["youtube"]);
}

#[test]
fn a_helper_that_steals_focus_is_caught_restored_and_quarantined() {
    let d = tempfile::tempdir().unwrap();
    let mut world = FakeWorld::typical(3);
    world.steal_focus = true;
    let (s, fake) = svc(d.path(), world);
    let v = run(&s, "every day close all my tabs");
    let st = &v["state"];
    assert_eq!(st["violations"], 1, "{v}");
    assert_eq!(
        st["quarantined"],
        json!([{"app": "com.apple.Safari", "rung": "scripting"}])
    );
    assert!(kinds(&v).iter().any(|k| k == "guard.restored"));
    // ctrl asked for Notes back.
    assert_eq!(fake.world().frontmost_pid, 100);
}

#[test]
fn a_failed_verify_backtracks_then_asks_when_out_of_alternatives() {
    let d = tempfile::tempdir().unwrap();
    let mut world = FakeWorld::typical(4);
    world.no_effect = true;
    let (s, fake) = svc(d.path(), world);
    // Slice 7b: a failed selective tab close asks instead of falling back to cmd-w (which can't skip pinned or
    // unsaved tabs): "close this tab" backtracks from the one-tab close to the selective close, which then asks.
    let text = "close this tab";
    let v = run(&s, text);
    let st = &v["state"];
    // Out of tries, or the re-decided move isn't sure: either way the user is asked.
    assert!(
        ["asked-out", "asking"].contains(&st["status"].as_str().unwrap()),
        "{v}"
    );
    let k = kinds(&v);
    let backtracks = k.iter().filter(|x| *x == "backtrack").count();
    assert!(backtracks >= 1, "{k:?}");
    let verifies: Vec<&Value> = st["verified"].as_array().unwrap().iter().collect();
    // Each failed verify backtracked; the last backtrack may land on a question for the user instead of a try.
    assert!(
        backtracks == verifies.len() || backtracks + 1 == verifies.len(),
        "{k:?}"
    );
    assert!(verifies.iter().all(|x| x["outcome"] == "failed"));
    // Every try was its own action with its own guard; none touched the user.
    assert_eq!(fake.world().acted.len(), verifies.len());
    untouched(&fake.world());
    replays(d.path(), &v, text, &["this tab"]);
}

#[test]
fn a_failed_selective_tab_close_asks_without_a_key_fallback() {
    let d = tempfile::tempdir().unwrap();
    let mut world = FakeWorld::typical(4);
    world.no_effect = true;
    let (s, fake) = svc(d.path(), world);
    let v = run(&s, "close all my tabs");
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert_eq!(fake.world().acted.len(), 1, "one try, no cmd-w fallback");
    assert_eq!(kinds(&v).iter().filter(|x| *x == "backtrack").count(), 0);
    untouched(&fake.world());
}

#[test]
fn an_irreversible_plan_stops_at_the_confirm_gate_before_acting() {
    let d = tempfile::tempdir().unwrap();
    let mut world = FakeWorld::typical(1);
    world
        .apps
        .push(FakeApp::new("com.apple.TextEdit", "TextEdit", 400));
    let (s, fake) = svc(d.path(), world);
    let v = run(&s, "every week quit textedit");
    let st = &v["state"];
    assert_eq!(st["status"], "needs-confirm", "{v}");
    assert_eq!(st["plan"]["irreversible"], true);
    assert_eq!(st["confirm"]["gateId"], "g0");
    assert!(fake.world().acted.is_empty());
    assert!(!kinds(&v).iter().any(|k| k == "action.requested"));
    // Emptying the trash can't be undone: still held after slice 7 (no batch expands it), nothing acted.
    let v = run(&s, "every week empty the trash");
    assert_eq!(v["state"]["held"], "files-not-yet", "{v}");
    assert!(fake.world().acted.is_empty());
}

#[test]
fn folder_cleanup_without_file_roots_is_held_and_executes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let (s, fake) = svc(d.path(), FakeWorld::typical(1));
    let v = run(&s, "every week clean up my downloads folder");
    assert_eq!(v["state"]["status"], "planned", "{v}");
    assert_eq!(v["state"]["held"], "files-not-yet");
    assert!(fake.world().acted.is_empty());
}

#[test]
fn cmd_l_is_never_sent_and_a_cold_app_is_never_launched() {
    // A browser that isn't running: opening a site needs the user's OK to bring it forward (Grant::Foreground),
    // so the run asks and nothing is acted.
    let d = tempfile::tempdir().unwrap();
    let mut world = FakeWorld::typical(0);
    world.default_browser = Some("com.google.Chrome".into());
    let (s, fake) = svc(d.path(), world);
    let v = run(&s, "open youtube");
    assert_eq!(v["state"]["status"], "asked-out", "{v}");
    assert_eq!(v["state"]["actions"], json!([{"id": "a0", "status": "asked"}]));
    let w = fake.world();
    assert!(w.acted.is_empty());
    assert!(
        !w.apps
            .iter()
            .find(|a| a.bundle_id == "com.google.Chrome")
            .unwrap()
            .running
    );
    untouched(&w);
}

fn textedit() -> FakeApp {
    let node =
        |id: &str, role: &str, sub: Option<&str>, title: &str, press: bool, children: u32| AxNodeWire {
            id: id.into(),
            role: role.into(),
            subrole: sub.map(str::to_string),
            title: title.into(),
            actions: if press { vec!["AXPress".into()] } else { vec![] },
            children,
        };
    let mut te = FakeApp::new("com.apple.TextEdit", "TextEdit", 400);
    te.ax_top = vec![
        node("e:close", "AXButton", Some("AXCloseButton"), "", true, 0),
        node("e:tools", "AXToolbar", None, "formatting", false, 2),
        node("e:menu", "AXPopUpButton", None, "styles", true, 0),
        node("e:text", "AXScrollArea", None, "", false, 1),
    ];
    te.ax_children.insert(
        "e:tools".into(),
        vec![
            node("e:bold", "AXCheckBox", None, "bold", true, 0),
            node("e:list", "AXButton", None, "bullet list", true, 0),
        ],
    );
    te
}

#[test]
fn the_front_windows_ax_branches_are_offered_next_to_the_shortcuts_and_an_answer_presses_one() {
    let d = tempfile::tempdir().unwrap();
    let mut world = FakeWorld::typical(0);
    world.apps.push(textedit());
    let (s, fake) = svc(d.path(), world);
    let text = "close the textedit window";
    let v = run(&s, text);
    let st = &v["state"];
    let mv = st["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["node"] == "choose_move")
        .expect("a move question")
        .clone();
    let ids: Vec<&str> = mv["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"route.scripting.close_window"), "{ids:?}");
    assert!(ids.contains(&"shortcut.cmd+w"), "{ids:?}");
    assert!(ids.contains(&"ax.e:close.close_button"), "{ids:?}");
    assert!(ids.contains(&"ax.e:tools.toolbar"), "{ids:?}");
    // A menu-opening element is never offered (ADR 0100 refuses it).
    assert!(!ids.iter().any(|i| i.contains("e:menu")), "{ids:?}");
    assert!(ids.len() <= 20);
    let close = mv["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "ax.e:close.close_button")
        .unwrap();
    assert_eq!(close["label"], "close button");
    // The user picks the close button; it is pressed in the background and verified by obs.diff.
    let mut v = v;
    // The script verb and the close button tie (TextEdit's dictionary isn't known to the heuristic layer), so the
    // run asks between them.
    assert_eq!(v["state"]["status"], "asking", "{v}");
    assert!(v["state"]["ask"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["id"] == "ax.e:close.close_button"));
    assert!(fake.world().acted.is_empty(), "nothing acts while asking");
    {
        v = s
            .call(
                "ctrl:answer",
                json!({"runId": v["run"]["id"], "decisionId": v["state"]["ask"]["decisionId"], "optionId": "ax.e:close.close_button"}),
            )
            .unwrap();
        assert_eq!(v["state"]["status"], "done", "{v}");
        let w = fake.world();
        assert!(w
            .acted
            .iter()
            .any(|(_, r)| matches!(r, Route::Ax { element, .. } if element.0 == "e:close")));
        assert!(kinds(&v).iter().any(|k| k == "obs.diff"));
        untouched(&w);
    }
    replays(d.path(), &v, text, &["textedit window", "formatting", "bullet"]);
}

#[test]
fn a_chosen_pane_is_observed_and_narrowed_to_its_controls() {
    use qrtz_ctrl::ax::AxSnapshot;
    use qrtz_ctrl::decide::Node;
    use qrtz_ctrl::layers::{app_entry, next_question_obs, AppCatalog, Next};
    use std::collections::BTreeMap;
    let mut cat = AppCatalog {
        apps: vec![app_entry("TextEdit")],
        default_browser: None,
        ax: true,
    };
    cat.apps[0].running = true;
    let apps = cat.observe();
    let goal = Goal {
        task_id: "task-1",
        text: "make it bold in textedit",
    };
    let mut chosen = BTreeMap::from([
        (Node::ChooseIntent, "intent.close_windows".to_string()),
        (Node::ChooseApp, "app.com.apple.TextEdit".to_string()),
    ]);
    // No snapshot yet: observe the front window first.
    assert_eq!(
        next_question_obs(&goal, &apps, &[], &chosen),
        Next::Observe {
            app: "com.apple.TextEdit".into(),
            scope: None
        }
    );
    let te = textedit();
    let top = AxSnapshot::from_wire("com.apple.TextEdit", None, &te.ax_top);
    assert_eq!(top.nodes.len(), 3, "the menu-opening pop-up is dropped");
    let Next::Ask(q) = next_question_obs(&goal, &apps, std::slice::from_ref(&top), &chosen) else {
        panic!("a move question")
    };
    assert_eq!(q.node, Node::ChooseMove);
    // The toolbar has children: choosing it observes them, then asks for a control inside it.
    chosen.insert(Node::ChooseMove, "ax.e:tools.toolbar".into());
    assert_eq!(
        next_question_obs(&goal, &apps, std::slice::from_ref(&top), &chosen),
        Next::Observe {
            app: "com.apple.TextEdit".into(),
            scope: Some("e:tools".into())
        }
    );
    let inner = AxSnapshot::from_wire("com.apple.TextEdit", Some("e:tools"), &te.ax_children["e:tools"]);
    let Next::Ask(q) = next_question_obs(&goal, &apps, &[top.clone(), inner.clone()], &chosen) else {
        panic!("a control question")
    };
    assert_eq!(q.node, Node::ChooseControl);
    // "bold" is in the goal: the bold checkbox ranks first.
    assert_eq!(q.choices[0].opt.id, "ax.e:bold.check_box");
    chosen.insert(Node::ChooseControl, "ax.e:bold.check_box".into());
    let Next::Plan(steps) = next_question_obs(&goal, &apps, &[top, inner], &chosen) else {
        panic!("a plan")
    };
    assert_eq!(steps.last().unwrap().node, Node::ChooseControl);
}

#[test]
fn the_ctrl_protocol_fixture_round_trips() {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../native/qrtz-helper/Fixtures/ctrl/protocol.jsonl"
    ))
    .unwrap();
    let (mut host, mut helper) = (0, 0);
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).unwrap();
        let t = v["type"].as_str().unwrap();
        let back = if t.ends_with(".result") || t == "ctrl.error" {
            helper += 1;
            serde_json::to_value(serde_json::from_str::<HelperMsg>(line).unwrap()).unwrap()
        } else {
            host += 1;
            serde_json::to_value(serde_json::from_str::<HostMsg>(line).unwrap()).unwrap()
        };
        assert_eq!(back, v, "{t} round-trips");
    }
    assert_eq!(
        (host, helper),
        (29, 11),
        "every message, twelve act routes (ADR 0130 quit_apps, ADR 0134 hide_apps, ADR 0135 mute_output, ADR 0137 set_dark_mode, ADR 0139 unmute_output, ADR 0140 set_volume, ADR 0166 open_folder, ADR 0168 clear_clipboard, ADR 0173 set_dock_autohide, ADR 0176 eject_volumes), the app-less output_muted, dark_mode, output_volume, clipboard_cleared (ADR 0168), dock_autohide (ADR 0173) and ejectable_volumes (ADR 0176) measures, the folder_open measure (ADR 0166), laya with and without a model, laya.warm (ADR 0133)"
    );
}

#[test]
fn without_a_helper_runs_still_stop_at_planned() {
    let d = tempfile::tempdir().unwrap();
    let s = CtrlService::open(d.path(), Box::new(Tick(AtomicU64::new(0)))).with_apps(Box::new(
        qrtz_ctrl::layers::FixedApps(qrtz_ctrl::layers::AppCatalog {
            apps: vec![qrtz_ctrl::layers::app_entry("Safari")],
            default_browser: Some("com.apple.Safari".into()),
            ax: false,
        }),
    ));
    let v = run(&s, "every day close all my tabs");
    assert_eq!(v["state"]["status"], "planned");
    assert!(!kinds(&v).iter().any(|k| k == "obs.snapshot"));
    let _ = Event::ExecHeld {
        reason: qrtz_ctrl::exec::HoldReason::Irreversible,
    };
}

#[test]
fn laya_runs_over_the_helper_protocol_and_falls_back_when_the_helper_has_no_model() {
    use qrtz_ctrl::decide::{ModelId, Scorer};
    use qrtz_ctrl::laya::{HelperLaya, LayaScorer};
    use qrtz_ctrl::layers::{app_entry, next_question, AppCatalog, Next};
    let mut world = FakeWorld::typical(1);
    let apps = AppCatalog {
        apps: vec![app_entry("Safari")],
        default_browser: Some("com.apple.Safari".into()),
        ax: false,
    }
    .observe();
    let goal = Goal {
        task_id: "task-1",
        text: "close all my tabs",
    };
    let Next::Ask(q) = next_question(&goal, &apps, &Default::default()) else {
        panic!()
    };
    let n = q.choices.len();
    let mut p = vec![0.0f32; n];
    p[1] = 1.0;
    world.laya = Some(p);
    let fake = Arc::new(FakeHelper::new(world));
    let scorer = LayaScorer {
        runtime: HelperLaya {
            helper: Arc::new(HelperClient::new(Box::new(fake.clone()))),
            model: ModelId {
                id: "laya".into(),
                version: "test".into(),
                quant: None,
            },
            path: Some("/models/ctrl/laya/laya.mlmodelc".into()),
        },
    };
    let l = scorer.logits(&q, &goal).unwrap();
    // ADR 0116: the consented model's path rides on every question.
    assert_eq!(
        fake.0.lock().unwrap().laya_models,
        [Some("/models/ctrl/laya/laya.mlmodelc".to_string())]
    );
    assert_eq!(l.iter().enumerate().max_by_key(|(_, x)| **x).unwrap().0, 1);
    // No model on the helper: an error, which the chain turns into the heuristic's answer.
    fake.0.lock().unwrap().laya = None;
    assert_eq!(scorer.logits(&q, &goal), Err("laya.no_model".into()));
}
