//! QRT-291: on every spawner path (Monolith, Split, Installed) a launch error that names the binary's path (the dev
//! stub's, a missing binary's) reaches neither `helper:status` (invoke or event) nor talk with a path in it.
use async_trait::async_trait;
use qrtz_app::module_helpers::{HelperLayout, ModuleHelpers};
use qrtz_app::ports::*;
use qrtz_app::*;
use qrtz_helper_bridge::{HelperState, HelperStatus, ProcessSpawn, SupervisorOptions};
use qrtz_ipc::{AppError, Router};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct NoHud;
impl ShellHooks for NoHud {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
}

/// Talk that keeps every `lastError` it was handed.
#[derive(Default)]
struct SeenTalk(Mutex<Vec<String>>);
#[async_trait]
impl TalkPort for SeenTalk {
    async fn call(&self, _c: &str, _p: Value) -> Result<Value, AppError> {
        Ok(Value::Null)
    }
    fn helper_line(&self, _m: &Value) {}
    fn helper_status(&self, s: &HelperStatus) {
        if let Some(e) = &s.last_error {
            self.0.lock().unwrap().push(e.clone());
        }
    }
    async fn toggle(&self) -> Result<Value, AppError> {
        Ok(Value::Null)
    }
}

fn opts() -> SupervisorOptions {
    SupervisorOptions {
        name: "t".into(),
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(2),
        heartbeat_timeout: Duration::ZERO,
        ..Default::default()
    }
}

fn dev_stub(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, b"#!/bin/sh\n# qrtz: dev stub\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn assert_no_path(what: &str, e: &str) {
    assert!(!e.contains('/') && !e.contains('\\'), "{what}: {e}");
    assert!(!e.contains("qrtz-helper-path-marker"), "{what}: {e}");
}

#[test]
fn every_spawner_arm_words_a_path_error_without_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let marked = dir.path().join("qrtz-helper-path-marker");
    std::fs::create_dir(&marked).unwrap();
    let stub = dev_stub(&marked, "qrtz-helper");
    let missing = marked.join("gone").join("qrtz-helper");
    for program in [stub.clone(), missing.clone()] {
        let base = ProcessSpawn::helper(program.clone());
        let mut programs = BTreeMap::new();
        programs.insert("echo".to_owned(), program.clone());
        for (name, layout) in [
            ("monolith", HelperLayout::Monolith),
            (
                "split",
                HelperLayout::Split {
                    programs: programs.clone(),
                },
            ),
            (
                "split-sibling",
                HelperLayout::Split {
                    programs: BTreeMap::new(),
                },
            ),
            (
                "installed",
                HelperLayout::Installed {
                    root: marked.clone(),
                    programs: programs.clone(),
                    bundled: BTreeMap::new(),
                },
            ),
        ] {
            let Err(e) = layout.spawner(&base, "echo").spawn() else {
                panic!("{name}: {program:?} started");
            };
            let e = e.to_string();
            assert_no_path(name, &e);
            assert!(e.starts_with("echo: "), "{name}: {e}");
        }
    }
}

#[test]
fn the_status_gate_rewords_any_path_left_in_last_error() {
    for raw in [
        "/tmp/x/qrtz-echo is the dev stub, not the real binary: run `bun run build:helper`",
        "echo: /Applications/qrtz.app/Contents/MacOS/qrtz-echo is missing",
        "nano: C:\\qrtz\\qrtz-nano: No such file or directory (os error 2)",
    ] {
        let s = installed::public_status(&HelperStatus {
            state: HelperState::Failed,
            restarts: 5,
            last_error: Some(raw.into()),
        });
        let e = s.last_error.unwrap();
        assert_no_path(raw, &e);
    }
    let fine = "echo: signature mismatch: the helper isn't signed with qrtz's Developer ID identity";
    assert_eq!(installed::public_error(fine), fine);
    assert_eq!(
        installed::public_error("exited code=1 signal=null"),
        "exited code=1 signal=null"
    );
}

async fn run_layout(name: &str, layout_of: impl Fn(&Path) -> (HelperLayout, PathBuf)) {
    let dir = tempfile::tempdir().unwrap();
    let marked = dir.path().join("qrtz-helper-path-marker");
    std::fs::create_dir(&marked).unwrap();
    let (layout, program) = layout_of(&marked);
    let layout_is_split = layout.is_split();
    let helpers = ModuleHelpers::spawn(&layout, ProcessSpawn::helper(program), opts());
    let echo = helpers.echo.clone();
    let talk = Arc::new(SeenTalk::default());
    let ev = Events::new();
    let mut events = ev.subscribe();
    let svc = AppService::new(AppDeps {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "test".into(),
        },
        config: ConfigStore::open(dir.path().join("config.json")).into(),
        runtime: Arc::new(StubRuntime::default()),
        storage: Arc::new(StubStorage),
        talk: talk.clone(),
        shell: Arc::new(NoHud),
        // The monolith runs as the service's one helper; split as one supervisor per module.
        helper: (!layout_is_split).then(|| echo.clone()),
        events: ev.clone(),
    });
    if layout_is_split {
        svc.set_module_helpers(helpers);
    }
    let r = Router::new(Shared(svc.clone()));
    svc.start_helper();
    if layout_is_split {
        echo.start();
    }
    let mut st = echo.watch_status();
    tokio::time::timeout(
        Duration::from_secs(10),
        st.wait_for(|s| s.state == HelperState::Failed),
    )
    .await
    .unwrap_or_else(|_| panic!("{name}: never gave up"))
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let status = r
        .dispatch(json!({"v":1,"id":"t","channel":"helper:status","version":1,"subApp":"settings"}))
        .await;
    let shown = status["data"]["lastError"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(!shown.is_empty(), "{name}: {status}");
    assert_no_path(name, &shown);
    let mut emitted = 0;
    while let Ok((ch, p)) = events.try_recv() {
        if ch == "helper:status" {
            emitted += 1;
            assert_no_path(name, &p.to_string());
        }
    }
    assert!(emitted > 0, "{name}: no helper:status event");
    let seen = talk.0.lock().unwrap().clone();
    assert!(!seen.is_empty(), "{name}: talk saw no error");
    for e in &seen {
        assert_no_path(name, e);
    }
}

#[tokio::test]
async fn monolith_dev_stub_reaches_status_and_talk_without_a_path() {
    run_layout("monolith", |d| {
        (HelperLayout::Monolith, dev_stub(d, "qrtz-helper"))
    })
    .await;
}

#[tokio::test]
async fn split_dev_stub_reaches_status_and_talk_without_a_path() {
    run_layout("split", |d| {
        let stub = dev_stub(d, "qrtz-echo");
        let programs = ["nano", "echo", "ctrl"]
            .iter()
            .map(|id| ((*id).to_owned(), stub.clone()))
            .collect();
        (HelperLayout::Split { programs }, d.join("qrtz-helper"))
    })
    .await;
}

#[tokio::test]
async fn split_missing_binary_reaches_status_and_talk_without_a_path() {
    run_layout("split-missing", |d| {
        (
            HelperLayout::Split {
                programs: BTreeMap::new(),
            },
            d.join("nowhere").join("qrtz-helper"),
        )
    })
    .await;
}
