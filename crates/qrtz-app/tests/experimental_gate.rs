//! QRT-352: QRT-349 hid nano and ctrl in the renderer only. The host must treat them as off too unless settings →
//! app → experimental apps is on, so a ctrl switched on in 0.3.2 or earlier never starts its helper after updating.
mod e2e_support;
use e2e_support::*;
use qrtz_helper_bridge::HelperState;
use std::time::Duration;

/// A data dir as 0.3.2 left it: ctrl (and nano) switched on, no `app.experimentalApps`.
fn seed_ctrl_on(data: &std::path::Path, app: Option<serde_json::Value>) {
    std::fs::create_dir_all(data).unwrap();
    let mut cfg = serde_json::json!({"modules": {"ctrl": {"enabled": true}, "nano": {"enabled": true}}});
    if let Some(a) = app {
        cfg["app"] = a;
    }
    std::fs::write(data.join("config.json"), cfg.to_string()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_left_on_from_0_3_2_does_not_start_without_experimental_apps() {
    for app in [None, Some(serde_json::json!({"experimentalApps": false}))] {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        seed_ctrl_on(&data, app.clone());
        let a = App::launch(&data, Opts::default().split_helpers().no_experimental_apps()).await;
        let m = a.app.service.module_helpers().expect("split helpers").clone();
        wait("echo running", || {
            let ok = m.echo.status().state == HelperState::Running;
            async move { ok.then_some(()) }
        })
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            m.ctrl.status().state,
            HelperState::Stopped,
            "ctrl started with {app:?}"
        );
        assert_eq!(
            m.nano.status().state,
            HelperState::Stopped,
            "nano started with {app:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_experimental_apps_on_ctrl_and_nano_start_as_before() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_ctrl_on(&data, Some(serde_json::json!({"experimentalApps": true})));
    let a = App::launch(&data, Opts::default().split_helpers()).await;
    let m = a.app.service.module_helpers().expect("split helpers").clone();
    wait("ctrl and nano running", || {
        let ok =
            m.ctrl.status().state == HelperState::Running && m.nano.status().state == HelperState::Running;
        async move { ok.then_some(()) }
    })
    .await;
}
