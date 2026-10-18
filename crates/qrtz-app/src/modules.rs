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
}
