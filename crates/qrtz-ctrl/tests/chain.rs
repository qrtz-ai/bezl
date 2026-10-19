//! The ctrl do chain end to end over fixture trees (Safari, Finder, System Settings, Notes, Mail, and Slack as a
//! poor tree), through the real helper protocol to a fake helper: pruning and grouping, act / tentative / ask,
//! backtracking, confirm gates bound to the tree, the stop key, replay, and a scan proving no fixture text is logged.
use qrtz_ctrl::decide::{Goal, Heuristic, ModelId, Node, Question, Scorer, MAX_OPTIONS};
use qrtz_ctrl::driver::{replay_check, Driver, Step};
use qrtz_ctrl::gate::ConfirmAnswer;
use qrtz_ctrl::guard::StopReason;
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::log::{Event, EventLog, TaskStatus};
use qrtz_ctrl::runner::Clock;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

fn app(name: &str) -> FakeApp {
    let p = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn world() -> FakeWorld {
    let apps: Vec<FakeApp> = ["safari", "notes", "finder", "settings", "mail", "slack"]
        .into_iter()
        .map(app)
        .collect();
    let mut w = FakeWorld {
        default_browser: Some("com.apple.Safari".into()),
        ..Default::default()
    };
    for a in &apps {
        if a.running {
            w.state.insert(a.bundle_id.clone(), a.start.clone());
        }
    }
    w.frontmost = Some("com.apple.finder".into());
    w.apps = apps;
    w
}

struct Tick(AtomicU64);
impl Clock for Tick {
    fn mono_ns(&self) -> u64 {
        self.0.fetch_add(1_000_000, Ordering::Relaxed)
    }
    fn wall_ms(&self) -> u64 {
        1_800_000_000_000
    }
}

/// A scripted scorer: the heuristic's priors plus a boost per option id.
struct Scripted(BTreeMap<String, i64>);
impl Scorer for Scripted {
    fn model(&self) -> ModelId {
        ModelId {
            id: "scripted".into(),
            version: "1".into(),
            quant: None,
        }
    }
    fn logits(&self, q: &Question, _g: &Goal) -> Result<Vec<i64>, String> {
        Ok(q.choices
            .iter()
            .map(|c| c.prior + self.0.get(&c.opt.id).copied().unwrap_or(0))
            .collect())
    }
}

struct Run {
    fake: Arc<FakeHelper>,
    client: HelperClient,
    log: EventLog,
    labels: BTreeMap<String, String>,
    goal: String,
    clock: Tick,
    stop: AtomicBool,
}

impl Run {
    fn new(goal: &str, w: FakeWorld) -> Run {
        let fake = Arc::new(FakeHelper::new(w));
        let client = HelperClient::new(Box::new(fake.clone()));
        let mut log = EventLog::new("r1");
        log.append(0, 0, Event::DoStarted { do_id: "do1".into() });
        Run {
            fake,
            client,
            log,
            labels: BTreeMap::new(),
            goal: goal.into(),
            clock: Tick(AtomicU64::new(0)),
            stop: AtomicBool::new(false),
        }
    }
    fn drive_with(&mut self, scorer: &dyn Scorer) -> Step {
        Driver {
            log: &mut self.log,
            helper: &self.client,
            scorer,
            clock: &self.clock,
            goal: &self.goal,
            labels: &mut self.labels,
            stop: &self.stop,
        }
        .drive()
    }
    fn drive(&mut self) -> Step {
        self.drive_with(&Heuristic)
    }
    fn push(&mut self, e: Event) {
        self.log.append(0, 0, e);
    }
    fn acts(&self) -> Vec<String> {
        self.fake
            .world()
            .acts
            .iter()
            .map(|a| {
                let k = a["action"].as_str().unwrap().to_string();
                match (a["handle"].as_str(), a["combo"].as_str(), a["app"].as_str()) {
                    (Some(h), _, _) => format!("{k}:{h}"),
                    (_, Some(c), _) => format!("{k}:{c}"),
                    (_, _, Some(app)) => format!("{k}:{app}"),
                    _ => k,
                }
            })
            .collect()
    }
    /// Every question the run asked had at most 20 options, "none of these" last.
    fn check_questions(&self) {
        for e in self.log.events() {
            if let Event::DecisionMade { options, .. } = e {
                assert!(options.len() <= MAX_OPTIONS, "{} options", options.len());
                assert_eq!(options.last().unwrap().id, "none");
            }
        }
    }
    fn finish_checks(&self) {
        self.log.verify().unwrap();
        self.check_questions();
        replay_check(&self.log, &self.goal).unwrap();
        scan_log(&self.log.to_jsonl(), &self.goal);
    }
}

/// Every title, value and placeholder in the fixtures, and the goal's words, never appear in the log.
fn scan_log(jsonl: &str, goal: &str) {
    // Host rule ids and role names are host text, not tree text: blank them before scanning.
    let mut text = String::new();
    for line in jsonl.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        collect_strings(&v, &mut text);
    }
    let lower = text.to_lowercase();
    let words: std::collections::BTreeSet<&str> = lower.split(|c: char| !c.is_alphanumeric()).collect();
    let mut needles: Vec<String> = Vec::new();
    for name in ["safari", "notes", "finder", "settings", "mail", "slack"] {
        let a = app(name);
        for st in a.states.values() {
            walk(st, &mut needles);
        }
        for m in &a.menu {
            needles.extend(m.path.iter().cloned());
        }
    }
    needles.extend(goal.split_whitespace().map(str::to_string));
    needles.push("youtube".into());
    needles.push("groceries".into());
    let mut roles = std::collections::BTreeSet::new();
    for name in ["safari", "notes", "finder", "settings", "mail", "slack"] {
        for st in app(name).states.values() {
            role_words(st, &mut roles);
        }
    }
    for n in needles {
        let n = n.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        let n = n.as_str().to_string();
        if n.chars().count() < 4 || n.contains("{text}") {
            continue;
        }
        if roles.contains(&n) || ["press", "focus", "scroll", "open"].contains(&n.as_str()) {
            continue; // role words ("list", "row") are host vocabulary, logged as roles
        }
        let hit = if n.contains(' ') {
            lower.contains(&n)
        } else {
            words.contains(n.as_str())
        };
        assert!(!hit, "fixture text {n:?} reached the log");
    }
}

fn collect_strings(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::String(s) => {
            // Bundle ids, roles, rule ids, kinds and option ids are host vocabulary.
            let host = s.starts_with("com.")
                || s.starts_with("AX")
                || s.starts_with("label:")
                || s.starts_with("app:")
                || s.starts_with("combo:")
                || s.contains('.') && !s.contains(' ')
                || s.contains('_') && !s.contains(' ')
                || s.contains('+') && !s.contains(' ');
            if !host {
                out.push_str(s);
                out.push('\n');
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

fn role_words(n: &qrtz_ctrl::ax::AxNodeWire, out: &mut std::collections::BTreeSet<String>) {
    for r in std::iter::once(&n.role).chain(n.subrole.as_ref()) {
        for w in qrtz_ctrl::ax::role_slug(r, None).split('_') {
            out.insert(w.to_string());
        }
    }
    for c in &n.children {
        role_words(c, out);
    }
}

fn walk(n: &qrtz_ctrl::ax::AxNodeWire, out: &mut Vec<String>) {
    out.push(n.title.clone());
    out.push(n.placeholder.clone());
    out.extend(n.value.clone());
    out.extend(n.title.split_whitespace().map(str::to_string));
    for c in &n.children {
        walk(c, out);
    }
}

fn decisions(log: &EventLog) -> Vec<(Node, String, String)> {
    log.events()
        .filter_map(|e| match e {
            Event::DecisionMade {
                node,
                chosen,
                outcome,
                ..
            } => Some((*node, chosen.clone(), format!("{outcome:?}").to_lowercase())),
            _ => None,
        })
        .collect()
}

#[test]
fn safari_open_youtube_goes_through_the_address_bar() {
    let mut r = Run::new("open youtube", world());
    assert_eq!(
        r.drive(),
        Step::Finished(TaskStatus::Done),
        "{:#?}",
        decisions(&r.log)
    );
    assert_eq!(
        r.acts(),
        vec!["open_app:com.apple.Safari", "key_combo:cmd+l", "set_value:e:10:4"]
    );
    let w = r.fake.world();
    assert_eq!(w.acts[2]["text"], "youtube.com");
    assert_eq!(w.acts[2]["submit"], true);
    let d = decisions(&r.log);
    assert_eq!(d[0].1, "app.com.apple.Safari");
    assert!(d.iter().any(|x| x.0 == Node::ChooseText && x.1 == "text.0.go"));
    assert_eq!(d.last().unwrap().1, "verify.done");
    r.finish_checks();
}

#[test]
fn notes_new_note_titled_types_the_title() {
    let mut r = Run::new("create a new note titled groceries", world());
    assert_eq!(
        r.drive(),
        Step::Finished(TaskStatus::Done),
        "{:#?}",
        decisions(&r.log)
    );
    let acts = r.acts();
    assert_eq!(acts[0], "open_app:com.apple.Notes");
    assert_eq!(acts.last().unwrap(), "set_value:e:20:6");
    assert_eq!(r.fake.world().acts.last().unwrap()["text"], "groceries");
    r.finish_checks();
}

/// ADR 0214 review (b): a move that ran but changed nothing is never backtracked (the action can't be undone by
/// choosing again). The run ends as failed and hands back; the second move is never tried. Both a tentative and a
/// confident (p >= 0.8) step.
fn notes_dead_menu_item() -> FakeWorld {
    let mut w = world();
    let notes = w
        .apps
        .iter_mut()
        .find(|a| a.bundle_id == "com.apple.Notes")
        .unwrap();
    notes.transitions.retain(|t| t.on != "press:m:20:1");
    w
}

#[test]
fn notes_hands_back_when_a_tentative_move_changes_nothing() {
    let mut r = Run::new("create a new note titled groceries", notes_dead_menu_item());
    let unsure = Scripted([("menu.m:20:1".to_string(), -1800)].into_iter().collect());
    assert_eq!(r.drive_with(&unsure), Step::Finished(TaskStatus::Failed));
    assert!(!r.log.events().any(|e| matches!(e, Event::Backtrack { .. })));
    assert!(!r.acts().contains(&"press:e:20:2".to_string()), "no second move");
}

#[test]
fn notes_hands_back_when_a_confident_move_fails_its_check() {
    let mut r = Run::new("create a new note titled groceries", notes_dead_menu_item());
    let step = r.drive();
    assert!(
        r.log.state().decisions.iter().any(|d| d.p_chosen_bp >= 8000),
        "{:#?}",
        decisions(&r.log)
    );
    assert_eq!(step, Step::Finished(TaskStatus::Failed));
    assert!(!r.log.events().any(|e| matches!(e, Event::Backtrack { .. })));
    assert!(!r.acts().contains(&"press:e:20:2".to_string()), "never continues");
}

#[test]
fn finder_groups_a_long_sidebar_and_descends() {
    let mut r = Run::new("open downloads in finder", world());
    let step = r.drive();
    // Twenty-five sidebar rows become two clusters; the downloads row is reached by descending.
    let mut answered = 0;
    let mut step = step;
    while step == Step::Asking && answered < 3 {
        let st = r.log.state();
        let d = st
            .decisions
            .iter()
            .find(|d| Some(&d.id) == st.ask.as_ref())
            .unwrap()
            .clone();
        let top = qrtz_ctrl::decide::ask_options(&d);
        let pick = d.options[top[0]].id.clone();
        r.push(Event::DecisionAnswered {
            decision_id: d.id.clone(),
            choice: Some(pick),
        });
        answered += 1;
        step = r.drive();
    }
    assert_eq!(step, Step::Finished(TaskStatus::Done), "{:#?}", decisions(&r.log));
    assert!(r.acts().contains(&"press:e:30:105".to_string()), "{:?}", r.acts());
    assert!(r
        .log
        .events()
        .any(|e| matches!(e, Event::DecisionMade { options, .. }
        if options.iter().any(|o| o.id.starts_with("group.row~")))));
    r.finish_checks();
}

#[test]
fn settings_changes_are_confirmed_and_the_scorer_cannot_skip_the_gate() {
    // A scorer that is sure of everything still hits the host's rule for System Settings changes.
    let sure = Scripted(
        [
            ("ax.e:40:10.press", 20_000),
            ("branch.e:40:2", 20_000),
            ("ax.e:40:3.press", 20_000),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect(),
    );
    let mut r = Run::new("turn on wi-fi in system settings", world());
    let mut step = r.drive_with(&sure);
    let mut gates = 0;
    while step == Step::Confirming {
        gates += 1;
        let g = r.log.state().confirm.unwrap();
        assert!(
            g.rules.contains(&"app:system_settings".to_string()),
            "{:?}",
            g.rules
        );
        r.push(Event::ConfirmResolved {
            gate_id: g.id,
            answer: ConfirmAnswer::Approve,
        });
        step = r.drive_with(&sure);
        assert!(gates < 6);
    }
    assert!(
        gates >= 2,
        "both the pane and the switch were confirmed: {gates} {:#?} {:?}",
        decisions(&r.log),
        r.log.state().status
    );
    assert!(
        r.acts().contains(&"press:e:40:3".to_string()),
        "{:?} {:#?}",
        r.acts(),
        decisions(&r.log)
    );
    // The password field in the pane never entered the tree.
    assert!(!r.log.to_jsonl().contains("e:40:4"));
    r.log.verify().unwrap();
    r.check_questions();
    scan_log(&r.log.to_jsonl(), &r.goal);
}

fn mail_send(spoof: bool, answer: ConfirmAnswer) -> Run {
    let mut w = world();
    if spoof {
        w.state.insert("com.apple.mail".into(), "draft_spoof".into());
        let mail = w
            .apps
            .iter_mut()
            .find(|a| a.bundle_id == "com.apple.mail")
            .unwrap();
        mail.menu.retain(|m| m.id != "m:50:1");
    }
    let handle = if spoof { "e:50:2" } else { "m:50:1" };
    let mut r = Run::new("press send in mail", w);
    assert_eq!(r.drive(), Step::Confirming, "{:#?}", decisions(&r.log));
    let g = r.log.state().confirm.unwrap();
    assert!(g.rules.contains(&"label:send".to_string()), "{:?}", g.rules);
    assert_eq!(g.binding.handle.as_deref(), Some(handle));
    assert!(
        !r.acts().iter().any(|a| a.starts_with("press")),
        "nothing pressed before the answer"
    );
    r.push(Event::ConfirmResolved {
        gate_id: g.id,
        answer,
    });
    r
}

#[test]
fn mail_send_waits_for_approval_then_acts_once() {
    let mut r = mail_send(false, ConfirmAnswer::Approve);
    assert_eq!(
        r.drive(),
        Step::Finished(TaskStatus::Done),
        "{:#?}",
        decisions(&r.log)
    );
    assert_eq!(r.acts().iter().filter(|a| *a == "press:m:50:1").count(), 1);
    assert!(r
        .log
        .events()
        .any(|e| matches!(e, Event::ConfirmReverified { ok: true, .. })));
    // The card-number field was excluded.
    assert!(!r.log.to_jsonl().contains("e:50:6"));
    r.finish_checks();
}

#[test]
fn a_homoglyph_send_is_still_gated() {
    let mut r = mail_send(true, ConfirmAnswer::Deny);
    assert_eq!(r.drive(), Step::Finished(TaskStatus::Cancelled));
    assert!(!r.acts().iter().any(|a| a.starts_with("press")));
}

#[test]
fn an_approval_is_void_when_the_target_changes_before_acting() {
    // Approve the toolbar button, then it changes under the approval (another fingerprint): asked again.
    let mut r = mail_send(true, ConfirmAnswer::Approve);
    r.fake
        .with(|w| w.state.insert("com.apple.mail".into(), "draft".into()));
    assert_eq!(r.drive(), Step::Confirming, "re-asked");
    assert!(r
        .log
        .events()
        .any(|e| matches!(e, Event::ConfirmReverified { ok: false, .. })));
    assert!(!r.acts().iter().any(|a| a.starts_with("press")));
}

/// Review M1: a full re-read gives out fresh ids (the real helper does, every time). The approval binds to the
/// element's fingerprint, not its id, so it still acts, once, on the same element under its new handle.
#[test]
fn an_approval_survives_a_re_read_with_fresh_ids() {
    fn renumber(n: &mut qrtz_ctrl::ax::AxNodeWire) {
        n.id = n.id.replace("e:50:", "e:77:");
        n.children.iter_mut().for_each(renumber);
    }
    let mut r = mail_send(true, ConfirmAnswer::Approve);
    r.fake.with(|w| {
        let mail = w
            .apps
            .iter_mut()
            .find(|a| a.bundle_id == "com.apple.mail")
            .unwrap();
        let mut fresh = mail.states["draft_spoof"].clone();
        renumber(&mut fresh);
        mail.states.insert("draft_fresh".into(), fresh);
        let mut t = mail.transitions[0].clone();
        t.on = "press:e:77:2".into();
        mail.transitions.push(t);
        w.state.insert("com.apple.mail".into(), "draft_fresh".into());
    });
    assert_eq!(
        r.drive(),
        Step::Finished(TaskStatus::Done),
        "{:#?}",
        decisions(&r.log)
    );
    assert_eq!(r.acts().iter().filter(|a| *a == "press:e:77:2").count(), 1);
    assert!(r
        .log
        .events()
        .any(|e| matches!(e, Event::ConfirmReverified { ok: true, handle: Some(h), .. } if h == "e:77:2")));
}

#[test]
fn slack_poor_tree_asks_and_a_cancel_ends_the_run() {
    let mut r = Run::new("message the design channel in slack", world());
    assert_eq!(r.drive(), Step::Asking, "{:#?}", decisions(&r.log));
    let st = r.log.state();
    let d = st
        .decisions
        .iter()
        .find(|d| Some(&d.id) == st.ask.as_ref())
        .unwrap();
    assert!(qrtz_ctrl::decide::ask_options(d).len() <= 3);
    let id = d.id.clone();
    r.push(Event::DecisionAnswered {
        decision_id: id,
        choice: None,
    });
    let _ = r.drive();
    r.push(Event::TaskFinished {
        status: TaskStatus::Cancelled,
        stop: None,
    });
    assert_eq!(r.log.state().status, Some(TaskStatus::Cancelled));
    r.check_questions();
    replay_check(&r.log, &r.goal).unwrap();
}

#[test]
fn the_stop_key_ends_the_run_and_nothing_more_is_acted() {
    let mut w = world();
    w.trip_after = Some((1, "stop_key".into()));
    let mut r = Run::new("open youtube", w);
    assert_eq!(r.drive(), Step::Finished(TaskStatus::Stopped));
    assert_eq!(r.log.state().stop, Some(StopReason::StopKey));
    assert_eq!(r.acts().len(), 1, "only the action that was already running");
}

/// Review H-A: Esc right after the ctrl+fn release (while ctrl reads the first window, before any action) stops
/// the run: the watcher is armed from the start of the drive, not at the first act.
#[test]
fn esc_before_the_first_action_stops_the_run() {
    let mut w = world();
    w.esc_on_catalog = true;
    let mut r = Run::new("open youtube", w);
    assert_eq!(r.drive(), Step::Finished(TaskStatus::Stopped));
    assert_eq!(r.log.state().stop, Some(StopReason::StopKey));
    assert!(r.acts().is_empty(), "{:?}", r.acts());
}

/// A stop the watcher still holds from a run whose host went away doesn't block the next run.
#[test]
fn a_stop_left_from_a_crashed_host_does_not_block_the_next_run() {
    let mut w = world();
    w.session = true;
    w.armed = true;
    w.tripped = Some("stop_key".into());
    let mut r = Run::new("open youtube", w);
    assert_eq!(r.drive(), Step::Finished(TaskStatus::Done));
}

#[test]
fn user_input_wins_and_no_watcher_means_no_action() {
    let mut w = world();
    w.trip_after = Some((2, "user_input".into()));
    let mut r = Run::new("open youtube", w);
    assert_eq!(r.drive(), Step::Finished(TaskStatus::Stopped));
    assert_eq!(r.log.state().stop, Some(StopReason::UserInput));
    assert_eq!(r.acts().len(), 2);

    let mut w = world();
    w.no_watcher = true;
    let mut r = Run::new("open youtube", w);
    assert_eq!(r.drive(), Step::Finished(TaskStatus::Stopped));
    assert_eq!(r.log.state().stop, Some(StopReason::NoWatcher));
    assert!(r.acts().is_empty());

    // Review (d): the stop says which permission is missing.
    for (m, want) in [
        ("accessibility", StopReason::NoAccessibility),
        ("input_monitoring", StopReason::NoInputMonitoring),
        ("secure_input", StopReason::SecureInput),
    ] {
        let mut w = world();
        w.no_watcher = true;
        w.missing = Some(m.into());
        let mut r = Run::new("open youtube", w);
        assert_eq!(r.drive(), Step::Finished(TaskStatus::Stopped));
        assert_eq!(r.log.state().stop, Some(want), "{m}");
        assert!(r.acts().is_empty());
    }
}

#[test]
fn the_stop_button_ends_the_run() {
    let mut r = Run::new("open youtube", world());
    r.stop.store(true, Ordering::SeqCst);
    assert_eq!(r.drive(), Step::Finished(TaskStatus::Stopped));
    assert_eq!(r.log.state().stop, Some(StopReason::StopButton));
}

#[test]
fn a_tampered_log_fails_replay() {
    let mut r = Run::new("open youtube", world());
    r.drive();
    // Another goal over the same log: the questions differ.
    assert!(replay_check(&r.log, "create a new note titled groceries").is_err());
}
