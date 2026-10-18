//! Run 19: the Laya scorer is re-selected per question. Consent + an installed model on disk + a runtime build →
//! the helper's Laya pass answers with that path; a download finishing or the user switching it off applies at the
//! next question, no restart; without a runtime build the heuristic always answers.
use qrtz_ctrl::decide::{Goal, Heuristic, ModelId, Scorer};
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient};
use qrtz_ctrl::laya::{LiveScorer, ModelConsent};
use qrtz_ctrl::layers::{next_question, AppCatalog, Next};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[test]
fn a_finished_download_or_an_off_applies_at_the_next_question() {
    let d = tempfile::tempdir().unwrap();
    let model_path = d.path().join("laya.mlmodelc");
    let consent = Arc::new(Mutex::new(ModelConsent::default()));
    let mut w = FakeWorld::typical(2);
    let goal = Goal {
        task_id: "t",
        text: "close all my tabs",
    };
    let Next::Ask(q) = next_question(&goal, &AppCatalog::default().observe(), &BTreeMap::new()) else {
        panic!("an intent question")
    };
    let n = q.choices.len();
    w.laya = Some(
        (0..n)
            .map(|i| if i == 0 { 0.9 } else { 0.1 / (n - 1) as f32 })
            .collect(),
    );
    let fake = Arc::new(FakeHelper::new(w));
    let c = consent.clone();
    let laya_id = ModelId {
        id: "laya".into(),
        version: "1".into(),
        quant: None,
    };
    let scorer = LiveScorer {
        consent: Box::new(move || c.lock().unwrap().clone()),
        helper: Arc::new(HelperClient::new(Box::new(fake.clone()))),
        model: laya_id.clone(),
        runtime_build: true,
    };
    // No consent yet: the heuristic answers and the helper is never asked.
    assert_eq!(scorer.model(), Heuristic.model());
    assert_eq!(scorer.logits(&q, &goal), Heuristic.logits(&q, &goal));
    // Consented, download not finished (no file): still the heuristic.
    *consent.lock().unwrap() = ModelConsent {
        consented: true,
        installed: Some(model_path.clone()),
    };
    assert_eq!(scorer.model(), Heuristic.model());
    assert!(fake.world().laya_models.is_empty());
    // The download lands: the very next question goes to Laya with the installed path.
    std::fs::create_dir_all(&model_path).unwrap();
    assert_eq!(scorer.model(), laya_id);
    let l = scorer.logits(&q, &goal).unwrap();
    assert_eq!(l.len(), n);
    assert_eq!(l[0], (0.9f64.ln() * 1000.0).round() as i64);
    assert_eq!(
        fake.world().laya_models,
        vec![Some(model_path.to_string_lossy().into_owned())]
    );
    // Switched off in settings: back to the heuristic at once, no new helper call.
    consent.lock().unwrap().consented = false;
    assert_eq!(scorer.model(), Heuristic.model());
    assert_eq!(scorer.logits(&q, &goal), Heuristic.logits(&q, &goal));
    assert_eq!(fake.world().laya_models.len(), 1);

    // A build without the runtime never asks the helper, whatever the consent says.
    consent.lock().unwrap().consented = true;
    let c2 = consent.clone();
    let off = LiveScorer {
        consent: Box::new(move || c2.lock().unwrap().clone()),
        helper: Arc::new(HelperClient::new(Box::new(fake.clone()))),
        model: laya_id,
        runtime_build: false,
    };
    assert_eq!(off.model(), Heuristic.model());
    assert_eq!(off.logits(&q, &goal), Heuristic.logits(&q, &goal));
    assert_eq!(fake.world().laya_models.len(), 1);
}
