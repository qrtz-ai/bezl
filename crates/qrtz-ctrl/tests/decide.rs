//! Slice 4 (ADR 0106): the decision layers. Each question has ≤ 20 options ending with "none of these"; the default
//! browser is ranked first; shortcuts sit next to routes (focus-intent ones flagged and scored down); clear tasks
//! act, unclear ones ask, middling ones go tentative and backtrack within budget; irreversible choices mark the
//! plan; the log holds no task text and replays to the same questions.
use qrtz_ctrl::decide::*;
use qrtz_ctrl::laya::{
    laya_request, select_scorer, Fallback, LayaRequest, LayaRuntime, LayaScorer, ModelConsent,
};
use qrtz_ctrl::layers::{app_entry, label, next_question, tokens, AppCatalog, AppsObs, Next};
use qrtz_ctrl::log::{apply, CtrlState, Event, EventLog};
use qrtz_ctrl::runner::Clock;

struct Still;
impl Clock for Still {
    fn mono_ns(&self) -> u64 {
        0
    }
    fn wall_ms(&self) -> u64 {
        0
    }
}

fn catalog(default: Option<&str>) -> AppsObs {
    AppCatalog {
        apps: [
            "Safari",
            "Google Chrome",
            "Firefox",
            "Finder",
            "System Settings",
            "Messages",
            "Slack",
            "Notes",
        ]
        .iter()
        .map(|n| app_entry(n))
        .collect(),
        default_browser: default.map(str::to_string),
    }
    .observe()
}

/// A run up to `intent.pending` + `obs.apps`, then the chain.
fn run(text: &str, apps: AppsObs) -> (EventLog, Vec<Event>) {
    run_with(text, apps, &Heuristic)
}

fn run_with(text: &str, apps: AppsObs, scorer: &dyn Scorer) -> (EventLog, Vec<Event>) {
    let mut log = EventLog::new("run-1");
    log.append(
        0,
        0,
        Event::TaskStarted {
            task_id: "task-0".into(),
        },
    );
    log.append(
        0,
        0,
        Event::IntentPending {
            task_id: "task-0".into(),
        },
    );
    log.append(0, 0, Event::ObsApps { apps });
    let goal = Goal {
        task_id: "task-0",
        text,
    };
    let events = run_chain(&goal, &log.state(), scorer, &Still);
    for e in &events {
        log.append(0, 0, e.clone());
    }
    (log, events)
}

fn decisions(events: &[Event]) -> Vec<(Node, String, Outcome, u32)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::DecisionMade {
                node,
                chosen,
                outcome,
                p_chosen_bp,
                ..
            } => Some((*node, chosen.clone(), *outcome, *p_chosen_bp)),
            _ => None,
        })
        .collect()
}

fn chosen(events: &[Event]) -> Vec<String> {
    decisions(events).into_iter().map(|d| d.1).collect()
}

fn options(e: &Event) -> &[Opt] {
    match e {
        Event::DecisionMade { options, .. } => options,
        _ => panic!("not a decision"),
    }
}

#[test]
fn goal_tokens_keep_their_spans() {
    let t = tokens("Open YouTube.com, then quit!");
    assert_eq!(
        t.iter().map(|t| t.w.as_str()).collect::<Vec<_>>(),
        ["open", "youtube.com", "then", "quit"]
    );
    assert_eq!((t[1].start, t[1].end), (5, 16));
}

#[test]
fn close_all_tabs_acts_at_every_node_with_the_default_browser_first() {
    let (log, ev) = run("every day close all my tabs", catalog(None));
    assert_eq!(
        chosen(&ev),
        [
            "intent.close_tabs",
            "app.default_browser",
            "scope.all_tabs",
            "route.scripting.close_all_tabs"
        ]
    );
    for (node, _, outcome, p) in decisions(&ev) {
        assert_eq!(outcome, Outcome::Act, "{node:?} p={p}");
        assert!(p >= 8000);
    }
    // The default browser is the first option; the other browsers follow; none of these is last.
    let app = options(&ev[1]);
    assert_eq!(app[0].id, "app.default_browser");
    assert!(app[1..4].iter().all(|o| o.kind == OptionKind::App));
    assert_eq!(app.last().unwrap().id, NONE);
    for e in &ev {
        if let Event::DecisionMade { options, .. } = e {
            assert!(options.len() <= MAX_OPTIONS);
            assert_eq!(options.last().unwrap().kind, OptionKind::None);
        }
    }
    let Event::PlanProduced {
        steps, irreversible, ..
    } = ev.last().unwrap()
    else {
        panic!("ends with a plan");
    };
    assert_eq!(steps.len(), 4);
    assert!(!irreversible);
    let s = log.state();
    assert!(s.plan.is_some() && !s.intent_pending && s.ask.is_none());
    assert_eq!(qrtz_ctrl::service::run_status(&s), "planned");
}

#[test]
fn a_known_default_browser_is_first_and_a_named_browser_wins() {
    let (_, ev) = run("close all my tabs", catalog(Some("com.google.Chrome")));
    assert_eq!(options(&ev[1])[0].id, "app.com.google.Chrome");
    assert_eq!(chosen(&ev)[1], "app.com.google.Chrome");
    let (_, ev) = run("close all my safari tabs", catalog(Some("com.google.Chrome")));
    let (_, app, outcome, _) = decisions(&ev)[1].clone();
    assert_eq!(app, "app.com.apple.Safari");
    assert_eq!(outcome, Outcome::Act);
}

#[test]
fn clean_up_downloads_is_a_files_plan() {
    let (_, ev) = run("every week clean up my downloads folder", catalog(None));
    assert_eq!(
        chosen(&ev),
        [
            "intent.clean_folder",
            "app.files",
            "folder.downloads",
            "route.files.tidy"
        ]
    );
    assert!(decisions(&ev).iter().all(|d| d.2 == Outcome::Act));
}

#[test]
fn open_youtube_offers_cmd_l_next_to_the_background_route() {
    let (_, ev) = run("open youtube", catalog(None));
    assert_eq!(
        chosen(&ev),
        [
            "intent.open_site",
            "app.default_browser",
            "text.url_guess",
            "route.workspace.open_url"
        ]
    );
    let goal = Goal {
        task_id: "task-0",
        text: "open youtube",
    };
    let text = options(&ev[2]);
    assert_eq!(label(&text[0], Some(&goal)), "youtube.com");
    assert_eq!(text[0].label_ref.as_deref(), Some("task-0#5..12"));
    assert_eq!(label(&text[1], Some(&goal)), "search: youtube");
    let Event::DecisionMade { options, p_bp, .. } = &ev[3] else {
        panic!()
    };
    let cmd_l = options
        .iter()
        .position(|o| o.id == "shortcut.cmd+l")
        .expect("cmd-l offered");
    assert_eq!(options[cmd_l].ladder, Some(LadderHint::FocusIntent));
    assert!(p_bp[cmd_l] < p_bp[0], "scored below the background route");
    assert_eq!(label(&options[cmd_l], None), "cmd-l focus address bar");
}

#[test]
fn an_unclear_task_asks_with_the_top_three() {
    let (log, ev) = run("do the thing", catalog(None));
    assert_eq!(ev.len(), 1);
    let (node, chosen, outcome, _) = decisions(&ev)[0].clone();
    assert_eq!(
        (node, chosen.as_str(), outcome),
        (Node::ChooseIntent, NONE, Outcome::Ask)
    );
    let s = log.state();
    assert_eq!(s.ask.as_deref(), Some("d0"));
    assert_eq!(qrtz_ctrl::service::run_status(&s), "asking");
    let top = ask_options(&s.decisions[0]);
    assert_eq!(top.len(), 3);
    assert!(top
        .iter()
        .all(|&i| s.decisions[0].options[i].kind != OptionKind::None));
    // The chain doesn't move while it waits.
    assert!(run_chain(
        &Goal {
            task_id: "task-0",
            text: "do the thing"
        },
        &s,
        &Heuristic,
        &Still
    )
    .is_empty());
    // The answer resumes it.
    let s = apply(
        s,
        &Event::DecisionAnswered {
            decision_id: "d0".into(),
            choice: Some("intent.empty_trash".into()),
        },
    );
    assert!(s.ask.is_none());
    let more = run_chain(
        &Goal {
            task_id: "task-0",
            text: "do the thing",
        },
        &s,
        &Heuristic,
        &Still,
    );
    let Event::PlanProduced { irreversible, .. } = more.last().unwrap() else {
        panic!("{more:?}")
    };
    assert!(irreversible, "emptying the trash is marked for the confirm gate");
}

#[test]
fn irreversible_choices_mark_the_plan() {
    let (_, ev) = run("empty the trash", catalog(None));
    assert_eq!(chosen(&ev)[0], "intent.empty_trash");
    assert!(matches!(
        ev.last(),
        Some(Event::PlanProduced {
            irreversible: true,
            ..
        })
    ));
    let (_, ev) = run("quit slack", catalog(None));
    assert_eq!(
        chosen(&ev)[..2],
        ["intent.quit_app", "app.com.tinyspeck.slackmacgap"]
    );
    assert!(matches!(
        ev.last(),
        Some(Event::PlanProduced {
            irreversible: true,
            ..
        })
    ));
}

#[test]
fn a_tentative_choice_backtracks_within_budget() {
    // "tabs" without "all": the scope is a coin-ish flip, so it goes tentative with alternatives.
    let (log, ev) = run("close tabs", catalog(None));
    let (_, scope, outcome, p) = decisions(&ev)[2].clone();
    assert_eq!(
        (scope.as_str(), outcome),
        ("scope.all_tabs", Outcome::Tentative),
        "p={p}"
    );
    let mut s = log.state();
    assert!(s.plan.is_some(), "tentative still plans");
    let b = next_backtrack(&s, BacktrackReason::VerifyFailed).expect("an alternative");
    let Event::Backtrack { decision_id, to, .. } = &b else {
        panic!()
    };
    assert_eq!((decision_id.as_str(), to.as_str()), ("d2", "scope.front_tab"));
    s = apply(s, &b);
    assert_eq!(s.decisions.len(), 3, "the move after it is dropped");
    assert!(s.plan.is_none());
    let goal = Goal {
        task_id: "task-0",
        text: "close tabs",
    };
    let more = run_chain(&goal, &s, &Heuristic, &Still);
    assert_eq!(chosen(&more), ["route.scripting.close_tab"]);
    assert!(matches!(more.last(), Some(Event::PlanProduced { .. })));
    for e in &more {
        s = apply(s, e);
    }
    // Ids stay unique across the backtrack.
    assert_eq!(s.decisions.last().unwrap().id, "d4");
    // Budget: 2 per decision. The scope has one alternative left, then none.
    let mut n = 1;
    while let Some(b) = next_backtrack(&s, BacktrackReason::VerifyFailed) {
        s = apply(s, &b);
        n += 1;
        assert!(n <= BACKTRACKS_PER_TASK);
    }
    assert!(s
        .decisions
        .iter()
        .all(|d| d.backtracks <= BACKTRACKS_PER_DECISION));
}

#[test]
fn options_are_capped_at_twenty() {
    let apps = AppCatalog {
        apps: (0..40).map(|i| app_entry(&format!("Tool {i}"))).collect(),
        default_browser: None,
    }
    .observe();
    let goal = Goal {
        task_id: "task-0",
        text: "open tool",
    };
    let mut chosen = std::collections::BTreeMap::new();
    chosen.insert(Node::ChooseIntent, "intent.open_app".to_string());
    let Next::Ask(q) = next_question(&goal, &apps, &chosen) else {
        panic!()
    };
    assert_eq!(q.node, Node::ChooseApp);
    assert_eq!(q.choices.len(), MAX_OPTIONS);
    assert_eq!(q.choices.last().unwrap().opt.id, NONE);
}

#[test]
fn the_log_is_text_free_deterministic_and_replays_to_the_same_questions() {
    let text = "every morning open youtube";
    let (log, ev) = run(text, catalog(None));
    let (_, again) = run(text, catalog(None));
    assert_eq!(ev, again, "same inputs, same decisions");
    let jsonl = log.to_jsonl();
    for word in ["youtube", "morning", "Firefox", "Messages"] {
        assert!(!jsonl.contains(word), "{word} leaked into the log");
    }
    // Round trip: same chain, same state, replay check passes.
    let loaded = EventLog::from_jsonl("run-1", &jsonl).unwrap();
    loaded.verify().unwrap();
    assert_eq!(loaded.state(), log.state());
    let goal = Goal {
        task_id: "task-0",
        text,
    };
    replay_check(&loaded, &goal).unwrap();
    // A different goal can't have produced these decisions: divergence at the first one.
    let other = Goal {
        task_id: "task-0",
        text: "every morning open github",
    };
    assert_eq!(replay_check(&loaded, &other), Err(Divergence { seq: 3 }));
    // Scrubbing folds without a scorer; a fork at the intent decision keeps it and drops the rest.
    assert_eq!(log.state_at(4).decisions.len(), 1);
    let f = log.fork(3, "run-2");
    assert_eq!(f.state().decisions.len(), 1);
    assert!(f.state().plan.is_none());
}

struct FakeLaya(Vec<f32>);
impl LayaRuntime for FakeLaya {
    fn model(&self) -> ModelId {
        ModelId {
            id: "laya-typed-decisions".into(),
            version: "test".into(),
            quant: Some("coreml".into()),
        }
    }
    fn choose(&self, r: &LayaRequest) -> Result<Vec<f32>, String> {
        let mut p = vec![0.0; r.options.len()];
        for (i, v) in self.0.iter().enumerate().take(p.len()) {
            p[i] = *v;
        }
        if self.0.is_empty() {
            return Err("no_model".into());
        }
        Ok(p)
    }
}

#[test]
fn a_laya_runtime_scores_through_the_same_chain_and_falls_back_when_it_fails() {
    let goal = Goal {
        task_id: "task-0",
        text: "close all my tabs",
    };
    let apps = catalog(None);
    let Next::Ask(q) = next_question(&goal, &apps, &Default::default()) else {
        panic!()
    };
    let req = laya_request(&q, &goal);
    assert_eq!(req.question, "what kind of task is this?");
    assert_eq!(req.options.len(), q.choices.len());
    // Options come in prior order: the heuristic's favourite first.
    assert_eq!(req.options[0], "close browser tabs");
    assert_eq!(req.options[1], "close windows");
    // The runtime says option 1 with 0.9: the chain follows the model, not the heuristic.
    let laya = LayaScorer {
        runtime: FakeLaya(vec![0.1, 0.9]),
    };
    let (_, ev) = run_with("close all my tabs", apps.clone(), &laya);
    let Event::DecisionMade {
        chosen,
        model,
        p_chosen_bp,
        ..
    } = &ev[0]
    else {
        panic!()
    };
    assert_eq!(chosen, "intent.close_windows");
    assert_eq!(model.id, "laya-typed-decisions");
    assert!((8900..=9100).contains(p_chosen_bp), "{p_chosen_bp}");
    // A failing runtime never blocks: the heuristic answers and the log names it.
    let broken = LayaScorer {
        runtime: FakeLaya(vec![]),
    };
    let (_, ev) = run_with("close all my tabs", apps, &broken);
    let Event::DecisionMade { chosen, model, .. } = &ev[0] else {
        panic!()
    };
    assert_eq!(
        (chosen.as_str(), model.id.as_str()),
        ("intent.close_tabs", HEURISTIC_ID)
    );
    // This build (Linux, no `laya` feature) always uses the heuristic layer.
    let (s, why) = select_scorer(
        &ModelConsent {
            consented: true,
            installed: Some("/".into()),
        },
        Some(Box::new(FakeLaya(vec![1.0]))),
    );
    assert_eq!(s.model().id, HEURISTIC_ID);
    assert_eq!(why, Some(Fallback::NotThisBuild));
}

#[test]
fn calibration_is_a_softmax_in_basis_points() {
    assert_eq!(softmax_bp(&[0, 0], 1000), [5000, 5000]);
    let p = softmax_bp(&[5000, 0, 0], 1000);
    assert!(p[0] > 9800);
    // A hotter temperature flattens.
    assert!(softmax_bp(&[5000, 0, 0], 3000)[0] < p[0]);
    let _ = CtrlState::default();
}
