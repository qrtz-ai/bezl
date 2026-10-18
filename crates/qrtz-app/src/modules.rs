//! ADR 0146 M1: the shell's module switches, applied. The bundled manifests and lockfile (`qrtz-modules`) are
//! loaded and checked once; a switched-off module's views can't call IPC, the channels only modules use are
//! refused while every module that uses them is off, its helper events stop reaching the host, and ctrl's scheduler
//! pauses. The switches default to on, so nothing changes until the user turns one off (settings → modules).
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_modules::lock::Lockfile;
use qrtz_modules::settings::GlobalSettings;
use qrtz_modules::ModuleManifest;
use serde_json::Value;
use std::sync::OnceLock;

/// Capabilities the shell itself needs whatever is installed: onboarding asks for permissions, and settings
/// manages the speech downloads.
const SHELL_SHARED: &[&str] = &["os.permissions", "speech.read", "speech.manage"];

pub struct Bundled {
    pub manifests: Vec<ModuleManifest>,
    pub lock: Lockfile,
}

/// The manifests and lockfile shipped in this build. A broken one is a build bug: the tests parse them.
pub fn bundled() -> &'static Bundled {
    static B: OnceLock<Bundled> = OnceLock::new();
    B.get_or_init(|| {
        let manifests = qrtz_modules::bundled::manifests()
            .iter()
            .map(|(id, s)| {
                ModuleManifest::parse(s.as_bytes()).unwrap_or_else(|e| panic!("bundled {id} manifest: {e}"))
            })
            .collect();
        let lock =
            Lockfile::parse(qrtz_modules::bundled::LOCK.as_bytes()).expect("bundled modules.lock.json");
        Bundled { manifests, lock }
    })
}

/// The module whose view is `sub_app` (`talk` → echo), or None for shell views.
pub fn module_of_view(sub_app: &str) -> Option<&'static str> {
    bundled()
        .manifests
        .iter()
        .find(|m| m.views.iter().any(|v| v == sub_app))
        .map(|m| m.id.as_str())
}

/// The modules that use `capability` (empty for shell capabilities and the shell-shared ones).
pub fn modules_using(capability: &str) -> Vec<&'static str> {
    if SHELL_SHARED.contains(&capability) {
        return vec![];
    }
    bundled()
        .manifests
        .iter()
        .filter(|m| m.capabilities.iter().any(|c| c == capability))
        .map(|m| m.id.as_str())
        .collect()
}

fn off(id: &str) -> AppError {
    AppError::new(
        ErrorCode::Unavailable,
        format!("{id} is switched off in settings → modules"),
    )
    .retryable(false)
}

/// The IPC gate: a switched-off module's view can't call anything but config and app info (so the switch can be
/// turned back on from anywhere), and a channel only modules use is refused while all of them are off.
pub fn gate(cfg: &Value, sub_app: &str, channel: &str) -> Result<(), AppError> {
    let g = GlobalSettings::from_config(cfg);
    if channel.starts_with("config:") || channel == "app:get-info" {
        return Ok(());
    }
    if let Some(m) = module_of_view(sub_app) {
        if !g.is_on(m) {
            return Err(off(m));
        }
    }
    if let Some(cap) = qrtz_ipc::contract()
        .channels
        .get(channel)
        .map(|d| d.capability.as_str())
    {
        let users = modules_using(cap);
        if !users.is_empty() && users.iter().all(|m| !g.is_on(m)) {
            return Err(off(users[0]));
        }
    }
    Ok(())
}

/// Helper events that only a switched-off module reacts to are dropped (meeting detection while nano is off).
pub fn drops_helper_event(cfg: &Value, ty: &str) -> bool {
    let g = GlobalSettings::from_config(cfg);
    // echo needs no filter: with echo off its talk config is off, so the helper doesn't dictate, and a dictation
    // already running still gets its end.
    ty.starts_with("meeting.") && !g.is_on("nano")
}

pub fn is_on(cfg: &Value, id: &str) -> bool {
    GlobalSettings::from_config(cfg).is_on(id)
}

/// One pass of ctrl's scheduler under the switches: None (nothing ticked, nothing recorded) while ctrl or all
/// modules are off, so a cadence that falls due meanwhile waits; back on, the next pass catches it up once
/// (ADR 0105's one catch-up, no burst).
pub fn ctrl_scheduler_pass(
    cfg: &Value,
    ctrl: &qrtz_ctrl::service::CtrlService,
) -> Option<Result<Vec<String>, qrtz_ctrl::service::CtrlError>> {
    is_on(cfg, "ctrl").then(|| ctrl.tick())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_bundled_modules_load_and_resolve() {
        let b = bundled();
        assert_eq!(b.manifests.len(), 3);
        for m in &b.manifests {
            b.lock.resolve(m).unwrap();
        }
        assert_eq!(module_of_view("talk"), Some("echo"));
        assert_eq!(module_of_view("note"), Some("nano"));
        assert_eq!(module_of_view("ctrl"), Some("ctrl"));
        assert_eq!(module_of_view("settings"), None);
    }

    #[test]
    fn everything_passes_with_the_default_config() {
        let cfg = json!({});
        for (app, ch) in [
            ("talk", "talk:state"),
            ("note", "note:list"),
            ("ctrl", "ctrl:tasks"),
            ("shell", "talk:state"),
        ] {
            assert!(gate(&cfg, app, ch).is_ok(), "{app} {ch}");
        }
    }

    #[test]
    fn a_switched_off_module_is_refused_but_config_stays_reachable() {
        let cfg = json!({"modules": {"echo": {"enabled": false}}});
        let e = gate(&cfg, "talk", "talk:state").unwrap_err();
        assert_eq!(e.code, ErrorCode::Unavailable);
        assert!(gate(&cfg, "talk", "config:get").is_ok());
        assert!(gate(&cfg, "note", "note:list").is_ok());
        // The shell's dictation (palette, hud) goes through echo's capability: refused too.
        assert!(gate(&cfg, "shell", "talk:state").is_err());
        // Speech setup is shared with settings.
        assert!(gate(&cfg, "settings", "speech:status").is_ok());
        let all = json!({"modules": {"enabled": false}});
        assert!(gate(&all, "ctrl", "ctrl:tasks").is_err());
        assert!(gate(&all, "settings", "config:set").is_ok());
    }

    #[test]
    fn helper_events_of_a_switched_off_module_are_dropped() {
        let cfg = json!({"modules": {"nano": {"enabled": false}}});
        assert!(drops_helper_event(&cfg, "meeting.detected"));
        assert!(!drops_helper_event(&cfg, "dictation.started"));
        assert!(!drops_helper_event(&json!({}), "meeting.detected"));
    }

    #[test]
    fn ctrl_scheduled_tasks_wait_while_ctrl_is_off_and_resume_when_it_is_back_on() {
        use qrtz_ctrl::service::{CtrlService, SystemClock};
        let d = tempfile::tempdir().unwrap();
        let svc = CtrlService::open(d.path(), Box::new(SystemClock::default()));
        // `once` with no time is due right away (ADR 0105).
        let t = svc
            .call(
                "ctrl:create-task",
                json!({"text": "close all my tabs", "schedule": {"kind": "once", "at": null}}),
            )
            .unwrap();
        let id = t["id"].as_str().unwrap().to_string();
        let runs = |s: &CtrlService| s.call("ctrl:tasks", json!(null)).unwrap()[0]["runs"].clone();
        for off in [
            json!({"modules": {"ctrl": {"enabled": false}}}),
            json!({"modules": {"enabled": false, "ctrl": {"enabled": true}}}),
        ] {
            assert!(ctrl_scheduler_pass(&off, &svc).is_none(), "{off}");
            assert_eq!(runs(&svc), 0, "nothing ran while off: {off}");
        }
        // Echo or nano off doesn't pause ctrl.
        let others_off = json!({"modules": {"echo": {"enabled": false}, "nano": {"enabled": false}}});
        let fired = ctrl_scheduler_pass(&others_off, &svc).unwrap().unwrap();
        assert_eq!(fired, vec![id]);
        assert_eq!(runs(&svc), 1);
        // A `once` that already ran doesn't run again when switched off and back on.
        assert!(ctrl_scheduler_pass(&json!({"modules": {"ctrl": {"enabled": false}}}), &svc).is_none());
        assert_eq!(
            ctrl_scheduler_pass(&json!({}), &svc).unwrap().unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(runs(&svc), 1);
    }
}
