//! ADR 0046 §1 / N55 end to end on Linux: `build_app` with the Rust mock helper; the on-device engine spawns the
//! real (confined) mock speech module installed from a signed bundle and hands its audio write ends to the helper
//! (`speech.attach` over the speech socket). The helper writes the dictation's mic PCM to them; when the dictation
//! stops it closes them, the module sees EOF, and the session finishes with the words it heard.
#![cfg(target_os = "linux")]
mod support;
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::AppInfo;
use qrtz_helper_bridge::{encode_host_message, HelperState, ProcessSpawn, SupervisorOptions};
use qrtz_speech::spawn::OnDeviceSpeech;
use qrtz_speech::{Channel, SessionKind, SessionSpec, SpeakerMode, SpeechEngine, SpeechError, SpeechEvent};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use support::speech::*;

struct Shell;
impl ShellHooks for Shell {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
    fn notify(&self, _n: Notice) {}
}

fn app(dir: &Path, helper: bool) -> BuiltApp {
    build_app(AppBuild {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        data_dir: dir.to_path_buf(),
        helper: helper.then(|| {
            (
                ProcessSpawn::helper(bin("qrtz-mock-helper", "qrtz-helper-bridge")),
                HelperAvailability::Simulated,
            )
        }),
        supervisor: SupervisorOptions::default(),
        shell: Arc::new(Shell),
        storage_shell: Arc::new(NoStorageShell),
        talk: None,
        speech: None,
        steps: None,
        knobs: DevKnobs::default(),
    })
    .unwrap()
}

fn spec(id: &str) -> SessionSpec {
    SessionSpec {
        session_id: id.into(),
        kind: SessionKind::Dictation,
        channels: vec![Channel::Mic],
        language_hint: None,
        speaker_mode: SpeakerMode::Fast,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_module_hears_the_helpers_mic_through_attached_fds_and_ends_on_eof() {
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await;
    env.install("1.0.0", "the quick brown fox").await;
    let a = app(&tmp.path().join("data"), true);
    a.service.start_helper();
    let h = a.service.helper().unwrap().clone();
    let mut w = h.watch_status();
    tokio::time::timeout(Duration::from_secs(10), async {
        while w.borrow_and_update().state != HelperState::Running {
            w.changed().await.unwrap();
        }
    })
    .await
    .unwrap();

    let engine = OnDeviceSpeech::new(env.store.clone(), env.opts.clone()).with_attach(a.attach.clone());
    let mut s = engine.start(spec("d1")).await.expect("attached");
    assert_eq!(env.store.active_sessions(), 1);
    assert!(
        h.send(encode_host_message(&json!({"type": "dictation.start", "id": "d1"})).unwrap())
            .await
    );
    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert!(
        h.send(encode_host_message(&json!({"type": "dictation.stop", "id": "d1"})).unwrap())
            .await
    );

    // EOF from the helper ends the audio; `finish` flushes. The host never held a write end.
    s.finish().await.unwrap();
    let mut evs = vec![];
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = s.events().recv().await {
            let end = matches!(ev, SpeechEvent::Finished { .. } | SpeechEvent::Failed { .. });
            evs.push(ev);
            if end {
                break;
            }
        }
    })
    .await;
    let finals: Vec<String> = evs
        .iter()
        .filter(|e| matches!(e, SpeechEvent::Words { is_final: true, .. }))
        .filter_map(|e| e.text())
        .collect();
    assert!(finals.len() >= 2, "one final per second of helper audio: {evs:?}");
    assert_eq!(finals[0], "the", "{evs:?}");
    assert!(
        matches!(evs.last(), Some(SpeechEvent::Finished { duration_ms }) if *duration_ms >= 2000),
        "{evs:?}"
    );
    h.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_helper_the_session_is_refused_and_the_module_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await;
    env.install("1.0.0", "the quick brown fox").await;
    let a = app(&tmp.path().join("data"), false);
    let engine = OnDeviceSpeech::new(env.store.clone(), env.opts.clone()).with_attach(a.attach.clone());
    let r = engine.start(spec("d2")).await;
    assert!(matches!(r, Err(SpeechError::Module(_))), "{:?}", r.err());
    // The cancelled module releases its pin.
    for _ in 0..100 {
        if env.store.active_sessions() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(env.store.active_sessions(), 0);
}
