#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! N129: a `models.warm` (ADR 0079, after a speech download) the helper couldn't take because it wasn't running is
//! remembered and sent on the next helper `hello`, once; `speech.warmAfterDownload` is read again then. The app's
//! helper-line tap (`wiring::helper_line_tap`) does it, and the hello still reaches the runtime and talk.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qrtz_app::ports::{NoShell, RuntimePort, StubStorage, StubTalk};
use qrtz_app::speech::HelperAttach;
use qrtz_app::wiring::{helper_line_tap, ModelsWarm};
use qrtz_app::{AppDeps, AppInfo, AppService, ConfigStore, Events};
use qrtz_helper_bridge::HelperStatus;
use qrtz_lifecycle::runtime::{HelperCommand, HelperPort};
use serde_json::Value;

const HELLO: &str =
    r#"{"features":[],"helperVersion":"0.1.0","os":"macOS 15.5","protocol":1,"type":"hello"}"#;

/// A helper that takes commands only while `up`.
#[derive(Default)]
struct Helper {
    up: AtomicBool,
    sent: Mutex<Vec<String>>,
}

impl HelperPort for Helper {
    fn send(&self, cmd: &HelperCommand) -> bool {
        if !self.up.load(Ordering::SeqCst) {
            return false;
        }
        self.sent.lock().unwrap().push(cmd.type_name().to_string());
        true
    }
}

impl Helper {
    fn warms(&self) -> usize {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|t| *t == "models.warm")
            .count()
    }
}

fn warm(h: &Arc<Helper>, enabled: &Arc<AtomicBool>) -> Arc<ModelsWarm> {
    let e = enabled.clone();
    Arc::new(ModelsWarm::new(
        h.clone(),
        Arc::new(move || e.load(Ordering::SeqCst)),
    ))
}

#[derive(Default)]
struct Lines(Mutex<Vec<String>>);

#[async_trait]
impl RuntimePort for Lines {
    async fn call(&self, _: &str, _: Value) -> Result<Value, qrtz_ipc::AppError> {
        Ok(Value::Null)
    }
    async fn toggle(&self) -> Result<Value, qrtz_ipc::AppError> {
        Ok(Value::Null)
    }
    fn helper_line(&self, m: &Value) {
        self.0
            .lock()
            .unwrap()
            .push(m["type"].as_str().unwrap_or_default().into());
    }
    fn helper_status(&self, _: &HelperStatus) {}
}

/// The app service with the app's tap, as `build_app` wires it.
fn service(w: Arc<ModelsWarm>, dir: &std::path::Path) -> (Arc<AppService>, Arc<Lines>) {
    let rt = Arc::new(Lines::default());
    let svc = AppService::new(AppDeps {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "darwin".into(),
            arch: "arm64".into(),
            shell: "tauri test".into(),
        },
        config: ConfigStore::open(dir.join("config.json")).into(),
        runtime: rt.clone(),
        storage: Arc::new(StubStorage),
        talk: Arc::new(StubTalk),
        shell: Arc::new(NoShell),
        helper: None,
        events: Events::new(),
    });
    svc.set_speech_tap(helper_line_tap(HelperAttach::new(None), None, Some(w)));
    (svc, rt)
}

#[test]
fn a_warm_while_the_helper_is_down_is_sent_on_its_next_hello_once() {
    let dir = tempfile::tempdir().unwrap();
    let (h, on) = (Arc::new(Helper::default()), Arc::new(AtomicBool::new(true)));
    let w = warm(&h, &on);
    let (svc, rt) = service(w.clone(), dir.path());
    w.request();
    assert_eq!(h.warms(), 0);
    assert!(w.pending());
    h.up.store(true, Ordering::SeqCst);
    svc.on_helper_line(HELLO);
    assert_eq!(h.warms(), 1, "sent on the hello");
    assert!(!w.pending());
    // The hello still reaches the runtime; a later helper process (restart) doesn't warm again.
    assert_eq!(*rt.0.lock().unwrap(), ["hello"]);
    svc.on_helper_line(HELLO);
    assert_eq!(h.warms(), 1);
}

#[test]
fn a_warm_the_helper_takes_is_not_sent_again_on_hello() {
    let dir = tempfile::tempdir().unwrap();
    let (h, on) = (Arc::new(Helper::default()), Arc::new(AtomicBool::new(true)));
    h.up.store(true, Ordering::SeqCst);
    let w = warm(&h, &on);
    let (svc, _) = service(w.clone(), dir.path());
    w.request();
    assert_eq!(h.warms(), 1);
    assert!(!w.pending());
    svc.on_helper_line(HELLO);
    assert_eq!(h.warms(), 1);
}

#[test]
fn a_hello_the_warm_still_cannot_reach_keeps_it_pending() {
    // The process said hello but stopped before the warm could be sent: the next hello tries again.
    let dir = tempfile::tempdir().unwrap();
    let (h, on) = (Arc::new(Helper::default()), Arc::new(AtomicBool::new(true)));
    let w = warm(&h, &on);
    let (svc, _) = service(w.clone(), dir.path());
    w.request();
    svc.on_helper_line(HELLO);
    assert!(w.pending());
    h.up.store(true, Ordering::SeqCst);
    svc.on_helper_line(HELLO);
    assert_eq!(h.warms(), 1);
    assert!(!w.pending());
}

#[test]
fn turning_the_setting_off_before_the_hello_drops_the_pending_warm() {
    let dir = tempfile::tempdir().unwrap();
    let (h, on) = (Arc::new(Helper::default()), Arc::new(AtomicBool::new(true)));
    let w = warm(&h, &on);
    let (svc, _) = service(w.clone(), dir.path());
    w.request();
    on.store(false, Ordering::SeqCst);
    h.up.store(true, Ordering::SeqCst);
    svc.on_helper_line(HELLO);
    assert_eq!(h.warms(), 0);
    assert!(!w.pending());
}

#[test]
fn with_the_setting_off_no_warm_is_sent_or_kept() {
    let (h, on) = (Arc::new(Helper::default()), Arc::new(AtomicBool::new(false)));
    let w = warm(&h, &on);
    w.request();
    assert!(!w.pending());
    h.up.store(true, Ordering::SeqCst);
    w.on_hello();
    assert_eq!(h.warms(), 0);
}
