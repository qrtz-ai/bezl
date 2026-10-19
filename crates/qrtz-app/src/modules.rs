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

/// Capabilities the shell's views need whatever is installed: the module manager shows and asks for permissions
/// beside each module's switch (ADR 0163), and settings and the marketplace manage the speech downloads and the
/// packages (ADR 0157, ADR 0161).
const SHELL_SHARED: &[&str] = &[
    "os.permissions",
    "speech.read",
    "speech.manage",
    "packages.read",
    "packages.manage",
];

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
    AppError::new(ErrorCode::Unavailable, format!("{id} is switched off in qrtz")).retryable(false)
}

/// The IPC gate: a switched-off module's view can't call anything but config and app info (so the switch can be
/// turned back on from anywhere), and a channel only modules use is refused while all of them are off.
pub fn gate(cfg: &Value, sub_app: &str, channel: &str) -> Result<(), AppError> {
    let g = GlobalSettings::from_config(cfg);
    if channel.starts_with("config:") || channel == "app:get-info" {
        return Ok(());
    }
    if let Some(m) = module_of_view(sub_app) {
        // ADR 0174: a bundled module's view stays in the sidebar while it is off and holds its own switch, so it
        // may read the status of the models switching it on would download.
        let bundled_status = qrtz_modules::is_bundled_extension(m) && channel == "speech:status";
        if !(g.is_on(m) || bundled_status) {
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

/// ADR 0174: a module view's `config:set` (only bundled echo's view holds `config.write`) may change exactly its own
/// module's switch, `{"modules": {"<id>": {"enabled": bool}}}`, and nothing else in config.
pub fn check_view_config_patch(sub_app: &str, channel: &str, payload: &Value) -> Result<(), AppError> {
    let Some(m) = module_of_view(sub_app) else {
        return Ok(());
    };
    if channel != "config:set" {
        return Ok(());
    }
    let patch = &payload["patch"];
    let only_switch = patch.as_object().is_some_and(|o| o.len() == 1)
        && patch["modules"].as_object().is_some_and(|o| o.len() == 1)
        && patch["modules"][m].as_object().is_some_and(|o| o.len() == 1)
        && patch["modules"][m]["enabled"].is_boolean();
    if only_switch {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::Validation,
            format!("{m}'s view can only switch {m} on or off"),
        )
        .retryable(false))
    }
}

/// ADR 0175 (N181): a module view's speech calls (only bundled echo's view holds `speech.manage`) reach only its
/// own models. `speech:download`, `speech:pause`, `speech:resume`, `speech:delete` and `speech:check` must name
/// `extensions == ["<id>"]` (omitted would mean every switched-on module, or the whole store), and the host scopes
/// each call to that set (`setup.rs` `*_for`). While the module is off, `speech:status` must name exactly it too.
/// Least privilege, not a confinement boundary: `sub_app` is the renderer's own claim.
pub fn check_view_speech_call(
    cfg: &Value,
    sub_app: &str,
    channel: &str,
    payload: &Value,
) -> Result<(), AppError> {
    let Some(m) = module_of_view(sub_app) else {
        return Ok(());
    };
    let scoped = matches!(
        channel,
        "speech:download" | "speech:pause" | "speech:resume" | "speech:delete" | "speech:check"
    ) || (channel == "speech:status" && !is_on(cfg, m));
    if !scoped {
        return Ok(());
    }
    let own = payload["extensions"]
        .as_array()
        .is_some_and(|a| a.len() == 1 && a[0].as_str() == Some(m));
    if own {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::Validation,
            format!("{m}'s view can only reach {m}'s own models (extensions: [\"{m}\"])"),
        )
        .retryable(false))
    }
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

/// ADR 0146 M1.4: what each module's own settings page reads and writes (`module:get-settings` /
/// `module:set-settings`): a whole config section, or one key of a section. Same table as
/// packages/core/src/modules.ts `MODULE_SETTINGS_PATHS` (the test below reads it). Storage (M2.3) follows it: `talk`
/// in echo's file; `note`, `speech.speakerMode` and `privacy` in nano's; the speech downloads stay in config.json
/// and the global settings (echo and nano share them; qrtz-modules `GLOBAL_KEYS`).
pub const MODULE_SETTINGS_PATHS: &[(&str, &str, Option<&str>)] = &[
    ("echo", "talk", None),
    ("nano", "note", None),
    ("nano", "speech", Some("speakerMode")),
    ("nano", "privacy", Some("excludedApps")),
    ("nano", "privacy", Some("pauseHotkey")),
    // ADR 0159: snap's shortcuts, status item and disabled apps (snap's settings file).
    ("snap", "snap", None),
];

fn settings_paths(module: &str) -> impl Iterator<Item = (&'static str, Option<&'static str>)> + '_ {
    MODULE_SETTINGS_PATHS
        .iter()
        .filter(move |(m, _, _)| *m == module)
        .map(|(_, s, k)| (*s, *k))
}

/// The module whose settings a view may use: the view's own module, or a refusal for shell views.
pub fn settings_module_of(sub_app: &str) -> Result<&'static str, AppError> {
    module_of_view(sub_app)
        .ok_or_else(|| AppError::new(ErrorCode::Forbidden, format!("{sub_app} is not an app view")))
}

/// The part of the merged config a module's settings page shows.
pub fn pick_settings(cfg: &Value, module: &str) -> Value {
    let mut out = serde_json::Map::new();
    for (section, key) in settings_paths(module) {
        let Some(v) = cfg.get(section).and_then(Value::as_object) else {
            continue;
        };
        let slot = out
            .entry(section)
            .or_insert_with(|| Value::Object(Default::default()));
        match key {
            None => *slot = Value::Object(v.clone()),
            Some(k) => {
                if let (Some(x), Some(o)) = (v.get(k), slot.as_object_mut()) {
                    o.insert(k.to_owned(), x.clone());
                }
            }
        }
    }
    Value::Object(out)
}

/// A patch from a module's settings page may only reach that module's settings.
pub fn check_settings_patch(module: &str, patch: &Value) -> Result<(), AppError> {
    let refuse = |what: String| Err(AppError::new(ErrorCode::Forbidden, what));
    let Some(p) = patch.as_object() else {
        return Err(AppError::new(ErrorCode::Validation, "patch must be an object"));
    };
    for (section, v) in p {
        let paths: Vec<_> = settings_paths(module).filter(|(s, _)| s == section).collect();
        if paths.iter().any(|(_, k)| k.is_none()) {
            continue;
        }
        if paths.is_empty() {
            return refuse(format!("{module} settings can't change {section}"));
        }
        let Some(o) = v.as_object() else {
            return Err(AppError::new(
                ErrorCode::Validation,
                format!("{section} must be an object"),
            ));
        };
        if let Some(bad) = o
            .keys()
            .find(|k| !paths.iter().any(|(_, x)| *x == Some(k.as_str())))
        {
            return refuse(format!("{module} settings can't change {section}.{bad}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_bundled_modules_load_and_resolve() {
        let b = bundled();
        assert_eq!(b.manifests.len(), 4);
        for m in &b.manifests {
            b.lock.resolve(m).unwrap();
        }
        assert_eq!(module_of_view("talk"), Some("echo"));
        assert_eq!(module_of_view("note"), Some("nano"));
        assert_eq!(module_of_view("ctrl"), Some("ctrl"));
        assert_eq!(module_of_view("snap"), Some("snap"));
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
        // ADR 0174: echo's view, while echo is off, may still read the speech status (to show Parakeet's size).
        {
            let mut off = cfg.clone();
            off["modules"]["echo"]["enabled"] = serde_json::json!(false);
            assert!(gate(&off, "talk", "speech:status").is_ok());
            assert!(gate(&off, "talk", "talk:history").is_err());
            assert!(gate(&off, "talk", "speech:download").is_err());
        }
        let only = |p: serde_json::Value| {
            check_view_config_patch("talk", "config:set", &serde_json::json!({ "patch": p }))
        };
        assert!(only(serde_json::json!({"modules": {"echo": {"enabled": true}}})).is_ok());
        assert!(only(serde_json::json!({"modules": {"nano": {"enabled": true}}})).is_err());
        assert!(only(serde_json::json!({"modules": {"enabled": true}})).is_err());
        assert!(only(serde_json::json!({"modules": {"echo": {"enabled": true}}, "theme": "dark"})).is_err());
        assert!(only(serde_json::json!({"talk": {"enabled": true}})).is_err());
        // Nested, aliased and array shapes can't reach another key.
        for p in [
            serde_json::json!({"modules": {"echo": {"enabled": true, "hosts": ["x"]}}}),
            serde_json::json!({"modules": {"echo": {"enabled": "true"}}}),
            serde_json::json!({"modules": {"echo": {"enabled": [true]}}}),
            serde_json::json!({"modules": [{"echo": {"enabled": true}}]}),
            serde_json::json!([{"modules": {"echo": {"enabled": true}}}]),
            serde_json::json!({"modules.nano.enabled": true}),
            serde_json::json!({"modules": {"echo.enabled": true}}),
            serde_json::json!({"modules": {"ctrl": {"enabled": false}}}),
            serde_json::json!({"hosts": {"modules": "x"}}),
            serde_json::json!(null),
        ] {
            assert!(only(p.clone()).is_err(), "{p}");
        }
        assert!(check_view_config_patch("talk", "config:set", &serde_json::json!({})).is_err());
        assert!(check_view_config_patch(
            "settings",
            "config:set",
            &serde_json::json!({"patch": {"theme": "dark"}})
        )
        .is_ok());
        let all = json!({"modules": {"enabled": false}});
        assert!(gate(&all, "ctrl", "ctrl:tasks").is_err());
        assert!(gate(&all, "settings", "config:set").is_ok());
    }

    #[test]
    fn a_module_view_reaches_only_its_own_models() {
        let on = json!({});
        let off = json!({"modules": {"echo": {"enabled": false}}});
        let own = json!({"extensions": ["echo"]});
        let bad = [
            json!({}),
            json!(null),
            json!({"force": true}),
            json!({"extensions": []}),
            json!({"extensions": ["nano"]}),
            json!({"extensions": ["ctrl"]}),
            json!({"extensions": ["echo", "nano"]}),
            json!({"extensions": ["echo", "echo"]}),
            json!({"extensions": "echo"}),
            json!({"extensions": [["echo"]]}),
        ];
        for ch in [
            "speech:download",
            "speech:pause",
            "speech:resume",
            "speech:delete",
            "speech:check",
        ] {
            for cfg in [&on, &off] {
                assert!(check_view_speech_call(cfg, "talk", ch, &own).is_ok(), "{ch}");
                assert!(check_view_speech_call(
                    cfg,
                    "talk",
                    ch,
                    &json!({"extensions": ["echo"], "force": true})
                )
                .is_ok());
                for b in &bad {
                    let e = check_view_speech_call(cfg, "talk", ch, b).unwrap_err();
                    assert_eq!(e.code, ErrorCode::Validation, "{ch} {b}");
                }
            }
            // Settings and the shell are not module views: unchanged (settings manages every model).
            for b in &bad {
                assert!(check_view_speech_call(&on, "settings", ch, b).is_ok(), "{ch} {b}");
                assert!(check_view_speech_call(&on, "shell", ch, b).is_ok(), "{ch} {b}");
            }
        }
        // speech:status: while echo is off its view may ask only about echo's models; on, unchanged.
        assert!(check_view_speech_call(&off, "talk", "speech:status", &own).is_ok());
        for b in &bad {
            assert!(
                check_view_speech_call(&off, "talk", "speech:status", b).is_err(),
                "{b}"
            );
            assert!(
                check_view_speech_call(&on, "talk", "speech:status", b).is_ok(),
                "{b}"
            );
        }
        assert!(check_view_speech_call(&off, "settings", "speech:status", &json!({})).is_ok());
        // Other channels pass through.
        assert!(check_view_speech_call(&off, "talk", "config:get", &json!(null)).is_ok());
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

    #[test]
    fn a_modules_settings_page_reaches_only_its_own_settings() {
        let cfg = json!({"theme": "dark", "talk": {"showPill": true}, "note": {"autoRecord": false},
            "speech": {"speakerMode": "fast", "autoUpdate": true},
            "privacy": {"excludedApps": ["Slack"], "pauseHotkey": "X"}, "modules": {"enabled": true}});
        assert_eq!(pick_settings(&cfg, "echo"), json!({"talk": {"showPill": true}}));
        assert_eq!(
            pick_settings(&cfg, "nano"),
            json!({"note": {"autoRecord": false}, "speech": {"speakerMode": "fast"},
                   "privacy": {"excludedApps": ["Slack"], "pauseHotkey": "X"}})
        );
        assert_eq!(pick_settings(&cfg, "ctrl"), json!({}));
        assert!(check_settings_patch("echo", &json!({"talk": {"showPill": false}})).is_ok());
        assert!(check_settings_patch(
            "nano",
            &json!({"note": {"autoRecord": true},
            "privacy": {"excludedApps": []}, "speech": {"speakerMode": "accurate"}})
        )
        .is_ok());
        for (m, p) in [
            ("echo", json!({"note": {"autoRecord": true}})),
            ("echo", json!({"modules": {"enabled": false}})),
            ("echo", json!({"theme": "light"})),
            ("nano", json!({"talk": {"showPill": false}})),
            ("nano", json!({"speech": {"autoUpdate": false}})),
            ("nano", json!({"privacy": {"other": 1}})),
            ("ctrl", json!({"talk": {}})),
        ] {
            let e = check_settings_patch(m, &p).unwrap_err();
            assert_eq!(e.code, ErrorCode::Forbidden, "{m} {p}");
        }
        assert_eq!(
            check_settings_patch("nano", &json!({"privacy": 1}))
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        assert_eq!(settings_module_of("talk").unwrap(), "echo");
        assert_eq!(
            settings_module_of("settings").unwrap_err().code,
            ErrorCode::Forbidden
        );
    }

    /// The renderer's table (packages/core/src/modules.ts) is this one.
    #[test]
    fn the_settings_paths_match_the_renderers() {
        let ts = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/core/src/modules.ts"),
        )
        .unwrap();
        let start = ts.find("export const MODULE_SETTINGS_PATHS").unwrap();
        let body = &ts[start..start + ts[start..].find("};").unwrap()];
        let mut n = 0;
        for id in qrtz_modules::MODULE_IDS {
            // The id's `[...]` value, matched by bracket depth.
            let at = body.find(&format!("{id}: [")).unwrap() + id.len() + 2;
            let mut depth = 0;
            let end = body[at..]
                .char_indices()
                .find(|&(_, c)| {
                    depth += match c {
                        '[' => 1,
                        ']' => -1,
                        _ => 0,
                    };
                    depth == 0
                })
                .unwrap()
                .0;
            let block = &body[at..=at + end];
            for (s, k) in settings_paths(id) {
                let want = match k {
                    None => format!("['{s}']"),
                    Some(k) => format!("['{s}', '{k}']"),
                };
                assert!(
                    block.contains(&want),
                    "{id}: {want} missing from the renderer's table: {block}"
                );
                n += 1;
            }
            assert_eq!(
                block.matches("['").count(),
                settings_paths(id).count(),
                "{id}: {block}"
            );
        }
        assert_eq!(n, MODULE_SETTINGS_PATHS.len());
    }
}
