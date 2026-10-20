//! qrtz snap (ADR 0159): the host side of qrtz-snap, Spectacle's window snapping as an extension helper.
//!
//! qrtz-snap is its own process in either helper layout (it shares nothing with the monolith): the host spawns it
//! directly, so it moves windows under qrtz's one Accessibility grant (ADR 0154). It runs exactly while snap is on
//! (`modules.snap` and the global switch). The host sends it `snap.configure` with snap's settings (config `snap`,
//! kept in snap's own settings file) whenever they change and on every (re)start; qrtz-snap answers `snap.status`
//! (the shortcuts it holds, the ones it couldn't, the Accessibility grant), which `snap:status` and the
//! `snap:changed` event hand to snap's view. Its status menu's "disable for <app>" comes back as `snap.disable-app`,
//! which becomes a change to `snap.disabledApps`.
use qrtz_helper_bridge::{HelperState, Supervisor};
use serde_json::{json, Value};
use std::sync::Mutex;

pub const MODULE_ID: &str = "snap";

/// The `snap.configure` line for this config (a missing section gives qrtz-snap Spectacle's defaults).
pub fn configure_line(cfg: &Value) -> String {
    let s = cfg.get("snap").cloned().unwrap_or_else(|| json!({}));
    let line = json!({
        "type": "snap.configure",
        "shortcuts": s.get("shortcuts").cloned().unwrap_or_else(|| json!({})),
        "disabledApps": s.get("disabledApps").cloned().unwrap_or_else(|| json!([])),
        "statusItem": s.get("statusItem").and_then(Value::as_bool).unwrap_or(true),
    });
    format!("{line}\n")
}

/// qrtz-snap's spawn: next to the helper binary, same environment plus `QRTZ_MODULE=snap` (no path override, so a
/// release build only ever runs the bundled helper). None for the Rust mock helper (Linux dev and tests), which has
/// no snap role.
pub fn spawn_for(base: &qrtz_helper_bridge::ProcessSpawn) -> Option<qrtz_helper_bridge::ProcessSpawn> {
    if base.program.file_name().and_then(|n| n.to_str()) == Some("qrtz-mock-helper") {
        return None;
    }
    Some(crate::module_helpers::module_spawn(
        base,
        MODULE_ID,
        &Default::default(),
    ))
}

/// qrtz-snap's spawner for `layout`: its installed package in the shipping app (ADR 0169), otherwise
/// [`spawn_for`]'s sibling of the helper.
pub fn spawner_for(
    layout: &crate::module_helpers::HelperLayout,
    base: &qrtz_helper_bridge::ProcessSpawn,
) -> Option<std::sync::Arc<dyn qrtz_helper_bridge::Spawn>> {
    match layout {
        crate::module_helpers::HelperLayout::Installed { .. } => Some(layout.spawner(base, MODULE_ID)),
        _ => spawn_for(base).map(|s| std::sync::Arc::new(s) as std::sync::Arc<dyn qrtz_helper_bridge::Spawn>),
    }
}

/// What a line from qrtz-snap asks of the host.
#[derive(Debug, PartialEq)]
pub enum SnapLine {
    /// A new status (already stored).
    Status,
    /// The config patch for "disable/enable for <app>".
    Patch(Value),
    /// Hello, move outcomes, anything else: nothing to do.
    None,
}

/// The `snap.disabledApps` patch that adds or removes `bundle_id`. None if nothing changes or the id is unusable.
pub fn disable_app_patch(cfg: &Value, bundle_id: &str, disabled: bool) -> Option<Value> {
    let id = bundle_id.trim();
    if id.is_empty() || id.len() > 255 {
        return None;
    }
    let mut apps: Vec<String> = cfg["snap"]["disabledApps"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let has = apps.iter().any(|a| a == id);
    match (disabled, has) {
        (true, false) => apps.push(id.to_owned()),
        (false, true) => apps.retain(|a| a != id),
        _ => return None,
    }
    Some(json!({"snap": {"disabledApps": apps}}))
}

/// qrtz-snap's supervisor and its last report.
pub struct SnapHost {
    pub sup: Supervisor,
    status: Mutex<Value>,
    /// The switch and the settings last applied, so an unrelated `config:set` changes nothing.
    applied: Mutex<Option<(bool, String)>>,
}

impl SnapHost {
    pub fn new(sup: Supervisor) -> Self {
        Self {
            sup,
            status: Mutex::new(Self::idle()),
            applied: Mutex::new(None),
        }
    }

    fn idle() -> Value {
        json!({"running": false, "axTrusted": false, "registered": {}, "refused": {}})
    }

    /// `snap:status`.
    pub fn status(&self) -> Value {
        self.status.lock().unwrap_or_else(crate::poison::recover).clone()
    }

    /// Start or stop qrtz-snap for the switch, and send the settings when they changed. True if anything changed.
    /// "On" is installed (ADR 0169: `modules.snap.installed`, from `qrtz.lock`) and switched on: a snap that isn't
    /// installed is stopped and never spawned, whatever its switch or settings say, so nothing retries it.
    pub fn apply(&self, cfg: &Value) -> bool {
        let g = qrtz_modules::settings::GlobalSettings::from_config(cfg);
        let on = g.is_installed(MODULE_ID) && g.is_on(MODULE_ID);
        let line = configure_line(cfg);
        let mut last = self.applied.lock().unwrap_or_else(crate::poison::recover);
        let was = last.clone();
        if was.as_ref() == Some(&(on, line.clone())) {
            return false;
        }
        *last = Some((on, line.clone()));
        drop(last);
        match (was.map(|w| w.0), on) {
            // Gave up after a crash loop (a dev stub, a missing grant): a change while on is the user's retry, like
            // switching snap off and on again (which starts it with a fresh restart budget).
            (Some(true), true) if self.sup.status().state == HelperState::Failed => self.sup.start(),
            (Some(true), true) => {
                self.sup.try_send(line);
            }
            (_, true) => self.sup.start(),
            (_, false) => {
                tracing::info!("snap is off: qrtz-snap not running");
                self.sup.stop();
                *self.status.lock().unwrap_or_else(crate::poison::recover) = Self::idle();
            }
        }
        true
    }

    /// A status change of qrtz-snap. On (re)start it gets the current settings; stopped, its status resets.
    pub fn on_state(&self, state: HelperState, cfg: &Value) -> bool {
        if state == HelperState::Running {
            self.sup.try_send(configure_line(cfg));
            false
        } else {
            let mut s = self.status.lock().unwrap_or_else(crate::poison::recover);
            let was_running = s["running"] == json!(true);
            *s = Self::idle();
            was_running
        }
    }

    pub fn on_line(&self, line: &str, cfg: &Value) -> SnapLine {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return SnapLine::None;
        };
        match v["type"].as_str() {
            Some("snap.status") => {
                let obj = |k: &str| match v.get(k) {
                    Some(Value::Object(o)) => Value::Object(o.clone()),
                    _ => json!({}),
                };
                *self.status.lock().unwrap_or_else(crate::poison::recover) = json!({
                    "running": true,
                    "axTrusted": v["axTrusted"].as_bool().unwrap_or(false),
                    "registered": obj("registered"),
                    "refused": obj("refused"),
                });
                SnapLine::Status
            }
            Some("snap.disable-app") => match (v["bundleId"].as_str(), v["disabled"].as_bool()) {
                (Some(id), Some(d)) => disable_app_patch(cfg, id, d).map_or(SnapLine::None, SnapLine::Patch),
                _ => SnapLine::None,
            },
            Some("snap.failed") => {
                tracing::debug!(reason = v["reason"].as_str().unwrap_or(""), "snap: nothing moved");
                SnapLine::None
            }
            _ => SnapLine::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_carries_snaps_settings_with_spectacles_defaults_when_unset() {
        let l: Value = serde_json::from_str(configure_line(&json!({})).trim()).unwrap();
        assert_eq!(
            l,
            json!({"type": "snap.configure", "shortcuts": {}, "disabledApps": [], "statusItem": true})
        );
        let cfg =
            json!({"snap": {"shortcuts": {"center": ""}, "disabledApps": ["com.a"], "statusItem": false}});
        let l: Value = serde_json::from_str(configure_line(&cfg).trim()).unwrap();
        assert_eq!(l["shortcuts"], json!({"center": ""}));
        assert_eq!(l["disabledApps"], json!(["com.a"]));
        assert_eq!(l["statusItem"], json!(false));
        assert!(configure_line(&cfg).ends_with('\n'));
    }

    #[test]
    fn qrtz_snap_runs_next_to_the_helper_and_never_from_the_mock() {
        use qrtz_helper_bridge::ProcessSpawn;
        let base = ProcessSpawn {
            program: "/Applications/qrtz.app/Contents/MacOS/qrtz-helper".into(),
            args: vec!["--stdio".into()],
            env: vec![],
        };
        let s = spawn_for(&base).unwrap();
        assert_eq!(
            s.program,
            std::path::PathBuf::from("/Applications/qrtz.app/Contents/MacOS/qrtz-snap")
        );
        assert!(s.env.contains(&("QRTZ_MODULE".to_owned(), "snap".to_owned())));
        let mock = ProcessSpawn {
            program: "/x/qrtz-mock-helper".into(),
            args: vec![],
            env: vec![],
        };
        assert!(spawn_for(&mock).is_none());
    }

    #[test]
    fn disable_app_adds_and_removes_once() {
        let cfg = json!({"snap": {"disabledApps": ["com.a"]}});
        assert_eq!(
            disable_app_patch(&cfg, "com.b", true),
            Some(json!({"snap": {"disabledApps": ["com.a", "com.b"]}}))
        );
        assert_eq!(disable_app_patch(&cfg, "com.a", true), None);
        assert_eq!(
            disable_app_patch(&cfg, "com.a", false),
            Some(json!({"snap": {"disabledApps": []}}))
        );
        assert_eq!(disable_app_patch(&cfg, "com.z", false), None);
        assert_eq!(disable_app_patch(&cfg, "  ", true), None);
    }

    fn host() -> SnapHost {
        use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
        let spawn = ProcessSpawn {
            program: "/nonexistent/qrtz-snap".into(),
            args: vec![],
            env: vec![],
        };
        SnapHost::new(Supervisor::spawn(
            SupervisorOptions::default(),
            std::sync::Arc::new(spawn),
        ))
    }

    #[tokio::test]
    async fn status_lines_fill_snap_status_and_a_stop_resets_it() {
        let h = host();
        assert_eq!(h.status()["running"], json!(false));
        let line = r#"{"type":"snap.status","axTrusted":true,"registered":{"leftHalf":"alt+cmd+left"},"refused":{"undo":"another app uses these keys"}}"#;
        assert_eq!(h.on_line(line, &json!({})), SnapLine::Status);
        let s = h.status();
        assert_eq!(s["running"], json!(true));
        assert_eq!(s["axTrusted"], json!(true));
        assert_eq!(s["registered"]["leftHalf"], json!("alt+cmd+left"));
        assert_eq!(s["refused"]["undo"], json!("another app uses these keys"));
        assert!(h.on_state(HelperState::Stopped, &json!({})));
        assert_eq!(h.status()["running"], json!(false));
        assert!(!h.on_state(HelperState::Stopped, &json!({})), "already reset");
    }

    #[tokio::test]
    async fn the_status_menu_toggle_becomes_a_settings_patch() {
        let h = host();
        let cfg = json!({"snap": {"disabledApps": []}});
        assert_eq!(
            h.on_line(
                r#"{"type":"snap.disable-app","bundleId":"com.x","disabled":true}"#,
                &cfg
            ),
            SnapLine::Patch(json!({"snap": {"disabledApps": ["com.x"]}}))
        );
        assert_eq!(
            h.on_line(r#"{"type":"snap.moved","action":"leftHalf"}"#, &cfg),
            SnapLine::None
        );
        assert_eq!(h.on_line("not json", &cfg), SnapLine::None);
    }

    /// A qrtz-snap that never starts (counted), on a supervisor that gives up after two quick tries.
    struct Broken(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl qrtz_helper_bridge::Spawn for Broken {
        fn spawn(&self) -> std::io::Result<qrtz_helper_bridge::ChildIo> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(std::io::Error::other("dev stub"))
        }
    }

    async fn until(sup: &Supervisor, state: HelperState) {
        let mut rx = sup.watch_status();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            rx.wait_for(|s| s.state == state),
        )
        .await
        .expect("state reached")
        .unwrap();
    }

    #[tokio::test]
    async fn switching_snap_back_on_or_changing_its_settings_retries_after_a_crash_loop() {
        let tries = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opts = qrtz_helper_bridge::SupervisorOptions {
            name: "qrtz-snap".into(),
            base_delay: std::time::Duration::from_millis(1),
            max_delay: std::time::Duration::from_millis(2),
            max_restarts: 2,
            heartbeat_timeout: std::time::Duration::ZERO,
            ..Default::default()
        };
        let h = SnapHost::new(Supervisor::spawn(
            opts,
            std::sync::Arc::new(Broken(tries.clone())),
        ));
        let n = || tries.load(std::sync::atomic::Ordering::SeqCst);
        let on = json!({"modules": {"enabled": true, "snap": {"enabled": true}}, "snap": {}});
        let off = json!({"modules": {"enabled": true, "snap": {"enabled": false}}, "snap": {}});
        assert!(h.apply(&on));
        until(&h.sup, HelperState::Failed).await;
        assert_eq!(n(), 3, "the first try and two restarts, then it gives up");
        assert!(h.apply(&off));
        until(&h.sup, HelperState::Stopped).await;
        assert!(h.apply(&on));
        until(&h.sup, HelperState::Failed).await;
        assert_eq!(n(), 6, "switched back on: a fresh restart budget");
        let mut changed = on.clone();
        changed["snap"]["statusItem"] = json!(false);
        assert!(h.apply(&changed));
        // Already Failed, so wait on the count rather than the state.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while n() < 9 || h.sup.status().state != HelperState::Failed {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("a settings change while it had given up is a retry too");
        assert_eq!(n(), 9);
        assert!(
            !h.apply(&changed),
            "an unrelated config:set still changes nothing"
        );
    }

    /// Design call (e): snap not installed means no spawn at all: not on launch, not when its switch or settings
    /// change, not after a stop. Installed, it starts; uninstalled, it stops and stays stopped.
    #[tokio::test]
    async fn a_snap_that_is_not_installed_is_never_spawned_or_retried() {
        let tries = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opts = qrtz_helper_bridge::SupervisorOptions {
            name: "qrtz-snap".into(),
            base_delay: std::time::Duration::from_millis(1),
            max_delay: std::time::Duration::from_millis(2),
            max_restarts: 2,
            heartbeat_timeout: std::time::Duration::ZERO,
            ..Default::default()
        };
        let h = SnapHost::new(Supervisor::spawn(
            opts,
            std::sync::Arc::new(Broken(tries.clone())),
        ));
        let n = || tries.load(std::sync::atomic::Ordering::SeqCst);
        let cfg = |installed: bool, enabled: bool, status_item: bool| {
            json!({"modules": {"enabled": true, "snap": {"enabled": enabled, "installed": installed}},
                   "snap": {"statusItem": status_item}})
        };
        h.apply(&cfg(false, true, true));
        h.apply(&cfg(false, true, false));
        h.apply(&cfg(false, false, false));
        h.apply(&cfg(false, true, true));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(n(), 0, "never spawned while not installed");
        assert_ne!(h.sup.status().state, HelperState::Failed);
        assert!(h.apply(&cfg(true, true, true)));
        until(&h.sup, HelperState::Failed).await;
        assert_eq!(
            n(),
            3,
            "installed: it starts (here: fails twice more, then gives up)"
        );
        assert!(h.apply(&cfg(false, true, true)));
        until(&h.sup, HelperState::Stopped).await;
        h.apply(&cfg(false, true, false));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            n(),
            3,
            "uninstalled: stopped, and a settings change doesn't retry it"
        );
    }

    #[tokio::test]
    async fn apply_acts_only_on_a_change_of_the_switch_or_snaps_settings() {
        let h = host();
        let on = json!({"modules": {"enabled": true, "snap": {"enabled": true}}, "snap": {}});
        assert!(h.apply(&on));
        assert!(!h.apply(&on), "an unrelated config:set changes nothing");
        let mut changed = on;
        changed["snap"]["statusItem"] = json!(false);
        assert!(h.apply(&changed));
        let off =
            json!({"modules": {"enabled": true, "snap": {"enabled": false}}, "snap": {"statusItem": false}});
        assert!(h.apply(&off));
        assert_eq!(h.status()["running"], json!(false));
    }
}
