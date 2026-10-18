//! ADR 0133: the Laya model is warmed on the helper as soon as it can be used. A tick sends `ctrl.laya.warm` only
//! with the helper up, a runtime build, consent and the installed model on disk, once per helper start and path;
//! a restart or a finished download warms again, an off never does, and a question afterwards still carries the
//! same path. The helper answers at once (its work runs on a background queue, see CtrlLayaWarmTests.swift).
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient, Wire};
use qrtz_ctrl::laya::{LayaWarmer, ModelConsent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The fake helper behind a switch, like the supervised process that may be restarting.
struct Switch {
    fake: Arc<FakeHelper>,
    up: AtomicBool,
}

impl Wire for Switch {
    fn roundtrip(&self, line: &str) -> Result<String, String> {
        if !self.up.load(Ordering::SeqCst) {
            return Err("helper.not_running".into());
        }
        self.fake.roundtrip(line)
    }
    fn up(&self) -> bool {
        self.up.load(Ordering::SeqCst)
    }
}

#[test]
fn warms_once_per_helper_start_only_with_consent_and_the_model_on_disk() {
    let d = tempfile::tempdir().unwrap();
    let model = d.path().join("laya_multilingual_e8_L512_options32.mlmodelc");
    let consent = Arc::new(Mutex::new(ModelConsent::default()));
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(2)));
    let wire = Arc::new(Switch {
        fake: fake.clone(),
        up: AtomicBool::new(false),
    });
    let c = consent.clone();
    let w = LayaWarmer::new(
        Box::new(move || c.lock().unwrap().clone()),
        Arc::new(HelperClient::new(Box::new(wire.clone()))),
        true,
    );
    let warms = || fake.world().laya_warms;

    // Helper not up yet: nothing sent, whatever the consent.
    assert_eq!(w.tick(), None);
    wire.up.store(true, Ordering::SeqCst);
    // Up, but no consent: never asked.
    assert_eq!(w.tick(), None);
    // Consented, download not finished: still nothing.
    *consent.lock().unwrap() = ModelConsent {
        consented: true,
        installed: Some(model.clone()),
    };
    assert_eq!(w.tick(), None);
    assert!(warms().is_empty());
    // The model lands: exactly one warm with the installed path, then quiet.
    std::fs::create_dir_all(&model).unwrap();
    assert_eq!(w.tick(), Some(Ok("started".into())));
    for _ in 0..5 {
        assert_eq!(w.tick(), None);
    }
    let p = model.to_string_lossy().into_owned();
    assert_eq!(warms(), vec![p.clone()]);
    // The helper restarts: warmed again once it's back.
    wire.up.store(false, Ordering::SeqCst);
    assert_eq!(w.tick(), None);
    wire.up.store(true, Ordering::SeqCst);
    assert_eq!(w.tick(), Some(Ok("warm".into())));
    assert_eq!(w.tick(), None);
    assert_eq!(warms(), vec![p.clone(), p.clone()]);
    // An off: nothing more is sent; a later yes + the file warms again.
    *consent.lock().unwrap() = ModelConsent::default();
    assert_eq!(w.tick(), None);
    assert_eq!(warms().len(), 2);
    *consent.lock().unwrap() = ModelConsent {
        consented: true,
        installed: Some(model.clone()),
    };
    assert!(w.tick().is_some());
    assert_eq!(warms().len(), 3);
    // The warm-up never asked a question or acted.
    let world = fake.world();
    assert!(world.laya_models.is_empty());
    assert!(world.acted.is_empty());
    assert!(world.opened_urls.is_empty());
}

#[test]
fn a_build_without_the_runtime_never_warms_and_a_failed_warm_is_not_retried_every_tick() {
    let d = tempfile::tempdir().unwrap();
    let model = d.path().join("laya.mlmodelc");
    std::fs::create_dir_all(&model).unwrap();
    let consent = ModelConsent {
        consented: true,
        installed: Some(model.clone()),
    };
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(2)));
    let c1 = consent.clone();
    let off = LayaWarmer::new(
        Box::new(move || c1.clone()),
        Arc::new(HelperClient::new(Box::new(fake.clone()))),
        false,
    );
    assert_eq!(off.tick(), None);
    assert!(fake.world().laya_warms.is_empty());

    // An older helper that doesn't know `ctrl.laya.warm`: asked once, the error is reported, no retry loop.
    struct Old;
    impl Wire for Old {
        fn roundtrip(&self, line: &str) -> Result<String, String> {
            let id = serde_json::from_str::<serde_json::Value>(line).unwrap()["id"].clone();
            Ok(serde_json::json!({"type": "ctrl.error", "id": id, "code": "helper.bad_request"}).to_string())
        }
    }
    let c2 = consent.clone();
    let old = LayaWarmer::new(
        Box::new(move || c2.clone()),
        Arc::new(HelperClient::new(Box::new(Old))),
        true,
    );
    assert_eq!(old.tick(), Some(Err("helper.bad_request".into())));
    assert_eq!(old.tick(), None);
}

#[test]
fn the_warm_up_waits_for_the_helper_to_settle_after_each_start() {
    let d = tempfile::tempdir().unwrap();
    let model = d.path().join("laya.mlmodelc");
    std::fs::create_dir_all(&model).unwrap();
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(2)));
    let wire = Arc::new(Switch {
        fake: fake.clone(),
        up: AtomicBool::new(true),
    });
    let m = model.clone();
    let w = LayaWarmer::new(
        Box::new(move || ModelConsent {
            consented: true,
            installed: Some(m.clone()),
        }),
        Arc::new(HelperClient::new(Box::new(wire.clone()))),
        true,
    )
    .with_settle(2);
    assert_eq!(w.tick(), None);
    assert_eq!(w.tick(), None);
    assert!(fake.world().laya_warms.is_empty());
    assert!(w.tick().is_some());
    assert_eq!(fake.world().laya_warms.len(), 1);
    // A restart settles again before the next warm-up.
    wire.up.store(false, Ordering::SeqCst);
    assert_eq!(w.tick(), None);
    wire.up.store(true, Ordering::SeqCst);
    assert_eq!(w.tick(), None);
    assert_eq!(w.tick(), None);
    assert!(w.tick().is_some());
    assert_eq!(fake.world().laya_warms.len(), 2);
}
