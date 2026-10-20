//! QRT-293: ctrl's decision model is required. There is no built-in rules scorer and no fallback: until the model is
//! downloaded, verified and loaded, ctrl reads nothing, decides nothing and acts on nothing, and says why (the
//! `ModelStatus` code `ctrl:model` reports for the "download to start" screen). One negative test per case Bozniak
//! listed: missing, mid-download, hash mismatch, load failure; plus an unsupported build, a model that answers
//! malformed, a model lost while a run waits on a confirm, and a ready model still held by the confirm gate (the
//! model only proposes).
use qrtz_ctrl::decide::{decide, Goal, ModelId, ModelStatus, NoModel, Question, Scorer};
use qrtz_ctrl::helper::{FakeApp, FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::laya::{LiveScorer, Readiness};
use qrtz_ctrl::runner::Clock;
use qrtz_ctrl::service::{CtrlErrorCode, CtrlService};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

struct Tick(AtomicU64);
impl Clock for Tick {
    fn mono_ns(&self) -> u64 {
        self.0.fetch_add(1_000_000, Ordering::Relaxed)
    }
    fn wall_ms(&self) -> u64 {
        1_800_000_000_000
    }
}

fn app(name: &str) -> FakeApp {
    let p = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn fake() -> Arc<FakeHelper> {
    let apps: Vec<FakeApp> = ["safari", "notes", "finder", "settings", "mail", "slack"]
        .into_iter()
        .map(app)
        .collect();
    let mut w = FakeWorld {
        default_browser: Some("com.apple.Safari".into()),
        frontmost: Some("com.apple.finder".into()),
        ..Default::default()
    };
    for a in &apps {
        if a.running {
            w.state.insert(a.bundle_id.clone(), a.start.clone());
        }
    }
    w.apps = apps;
    Arc::new(FakeHelper::new(w))
}

fn service(dir: &std::path::Path, scorer: Option<Box<dyn Scorer>>) -> (CtrlService, Arc<FakeHelper>) {
    let fake = fake();
    let mut svc = CtrlService::open(dir, Box::new(Tick(AtomicU64::new(0))))
        .with_helper(Arc::new(HelperClient::new(Box::new(fake.clone()))));
    if let Some(s) = scorer {
        svc = svc.with_scorer(s);
    }
    (svc, fake)
}

fn model() -> ModelId {
    ModelId {
        id: "laya".into(),
        version: "1.0.0".into(),
        quant: None,
    }
}

/// The app's scorer over a fake helper, with the host's readiness fixed to `r`.
fn live(r: Readiness, runtime_build: bool) -> Box<dyn Scorer> {
    Box::new(LiveScorer::new(
        Box::new(move || r.clone()),
        Arc::new(HelperClient::new(Box::new(fake()))),
        model(),
        runtime_build,
    ))
}

/// A ctrl:do is refused with `code` before anything is stored, read from the tree, or acted on.
fn assert_refused(svc: &CtrlService, fake: &FakeHelper, code: &str) {
    let e = svc
        .call("ctrl:do", json!({ "text": "open youtube" }))
        .unwrap_err();
    assert_eq!(e.code, CtrlErrorCode::Unavailable, "{e:?}");
    assert!(e.message.contains(&format!("model.{code}")), "{}", e.message);
    assert!(fake.world().acts.is_empty(), "no action");
    assert!(
        !fake.world().armed,
        "the stop-key watcher was never armed: nothing ran"
    );
    assert_eq!(
        svc.call("ctrl:runs", json!(null)).unwrap(),
        json!([]),
        "no run recorded"
    );
    assert_eq!(svc.model_status().code(), code);
}

#[test]
fn with_no_model_wired_in_ctrl_refuses_as_not_downloaded() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, fake) = service(dir.path(), None);
    assert_refused(&svc, &fake, "not_downloaded");
}

#[test]
fn a_missing_model_takes_no_action() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, fake) = service(
        dir.path(),
        Some(live(Readiness::not(ModelStatus::NotDownloaded), true)),
    );
    assert_refused(&svc, &fake, "not_downloaded");
    // A host that claims ready but hands no path (or a path that's gone) is still not downloaded.
    let gone = PathBuf::from("/nonexistent/qrtz/laya.mlmodelc");
    for r in [
        Readiness {
            status: ModelStatus::Ready,
            path: None,
        },
        Readiness {
            status: ModelStatus::Ready,
            path: Some(gone),
        },
    ] {
        let d = tempfile::tempdir().unwrap();
        let (svc, fake) = service(d.path(), Some(live(r, true)));
        assert_refused(&svc, &fake, "not_downloaded");
    }
}

#[test]
fn a_model_mid_download_takes_no_action() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, fake) = service(
        dir.path(),
        Some(live(Readiness::not(ModelStatus::Downloading), true)),
    );
    assert_refused(&svc, &fake, "downloading");
}

#[test]
fn a_hash_mismatch_takes_no_action() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, fake) = service(
        dir.path(),
        Some(live(Readiness::not(ModelStatus::HashMismatch), true)),
    );
    assert_refused(&svc, &fake, "hash_mismatch");
}

#[test]
fn a_build_without_the_runtime_takes_no_action_even_with_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    let r = Readiness {
        status: ModelStatus::Ready,
        path: Some(file.path().to_path_buf()),
    };
    let (svc, fake) = service(dir.path(), Some(live(r, false)));
    assert_refused(&svc, &fake, "unavailable");
}

#[test]
fn a_load_failure_takes_no_action_and_reports_load_failed() {
    // The files verified, but the helper can't load the model (the fake answers `laya.no_model` to every
    // `ctrl.laya`): the run ends at its first question with no action, and the status says load_failed.
    let dir = tempfile::tempdir().unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    let r = Readiness {
        status: ModelStatus::Ready,
        path: Some(file.path().to_path_buf()),
    };
    let (svc, fake) = service(dir.path(), Some(live(r, true)));
    assert_eq!(svc.model_status(), ModelStatus::Ready);
    let v = svc.call("ctrl:do", json!({ "text": "open youtube" })).unwrap();
    assert_eq!(v["status"], "failed", "{v:#}");
    assert_eq!(v["decisions"], json!([]), "nothing decided");
    assert_eq!(v["actions"], json!([]), "no action");
    assert!(fake.world().acts.is_empty());
    assert_eq!(svc.model_status(), ModelStatus::LoadFailed);
    // The next command tries the model again, and fails closed again.
    let v = svc.call("ctrl:do", json!({ "text": "open youtube" })).unwrap();
    assert_eq!(
        (v["status"].as_str(), v["actions"].as_array().map(Vec::len)),
        (Some("failed"), Some(0))
    );
    assert!(fake.world().acts.is_empty());
    assert_eq!(svc.model_status(), ModelStatus::LoadFailed);
}

/// A model that is ready but answers wrong: an error, or the wrong number of logits.
struct Broken(bool);
impl Scorer for Broken {
    fn model(&self) -> ModelId {
        model()
    }
    fn logits(&self, q: &Question, _g: &Goal) -> Result<Vec<i64>, String> {
        if self.0 {
            Err("laya.load_failed".into())
        } else {
            Ok(vec![0; q.choices.len() + 1])
        }
    }
    fn status(&self) -> ModelStatus {
        ModelStatus::Ready
    }
}

#[test]
fn a_failing_or_malformed_model_decides_nothing() {
    for err in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (svc, fake) = service(dir.path(), Some(Box::new(Broken(err))));
        let v = svc.call("ctrl:do", json!({ "text": "open youtube" })).unwrap();
        assert_eq!(v["status"], "failed", "{v:#}");
        assert_eq!(v["decisions"], json!([]));
        assert!(fake.world().acts.is_empty());
    }
}

#[test]
fn decide_with_no_ready_model_is_an_error_not_a_fallback() {
    use qrtz_ctrl::decide::{Choice, Node, Opt, OptionKind};
    let q = Question {
        node: Node::ChooseApp,
        choices: vec![
            Choice {
                opt: Opt::new("a", OptionKind::App),
                prior: 9000,
                label: "a".into(),
            },
            Choice {
                opt: Opt::none(),
                prior: 1000,
                label: String::new(),
            },
        ],
    };
    let g = Goal { text: "open a" };
    for s in [
        ModelStatus::NotDownloaded,
        ModelStatus::Downloading,
        ModelStatus::HashMismatch,
        ModelStatus::LoadFailed,
        ModelStatus::Unavailable,
    ] {
        let e = decide(&q, &g, "p", &NoModel(s), &Tick(AtomicU64::new(0)), "d0".into()).unwrap_err();
        assert_eq!(e, format!("model.{}", s.code()));
    }
}

/// The test stand-in for a ready model (the option sources' ranks as logits), switchable off mid-run.
struct Switch(Arc<AtomicBool>);
impl Scorer for Switch {
    fn model(&self) -> ModelId {
        model()
    }
    fn logits(&self, q: &Question, _g: &Goal) -> Result<Vec<i64>, String> {
        Ok(q.choices.iter().map(|c| c.prior).collect())
    }
    fn status(&self) -> ModelStatus {
        if self.0.load(Ordering::SeqCst) {
            ModelStatus::Ready
        } else {
            ModelStatus::NotDownloaded
        }
    }
}

#[test]
fn the_model_only_proposes_the_confirm_gate_still_holds_and_a_lost_model_acts_on_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let on = Arc::new(AtomicBool::new(true));
    let (svc, fake) = service(dir.path(), Some(Box::new(Switch(on.clone()))));
    // A ready model picks "send": the gate (host-owned, unchanged) still holds it for Jared.
    let v = svc
        .call("ctrl:do", json!({ "text": "press send in mail" }))
        .unwrap();
    assert_eq!(v["status"], "needs-confirm", "{v:#}");
    assert_eq!(v["confirm"]["rules"], json!(["label:send"]));
    assert!(fake.world().acts.iter().all(|a| a["action"] != "press"));
    // The model goes away (consent withdrawn, files deleted) while the run waits: approving acts on nothing.
    on.store(false, Ordering::SeqCst);
    let run = v["runId"].as_str().unwrap();
    let gate = v["confirm"]["gateId"].as_str().unwrap();
    let v = svc
        .call(
            "ctrl:confirm",
            json!({ "runId": run, "gateId": gate, "answer": "approve" }),
        )
        .unwrap();
    assert_eq!(v["status"], "failed", "{v:#}");
    assert!(
        fake.world().acts.iter().all(|a| a["action"] != "press"),
        "send was never pressed"
    );
}
