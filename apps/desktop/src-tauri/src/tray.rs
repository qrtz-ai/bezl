//! Menu bar item. `tray_state` is a port of apps/desktop/src/main/tray-model.ts (pure, unit-tested); the
//! rest wires it to Tauri's tray + menu.
use crate::strings::*;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct TrayItem {
    pub label: String,
    pub action: Option<&'static str>,
    pub enabled: bool,
    pub separator: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrayState {
    pub recording: bool,
    pub title: String,
    pub tooltip: String,
    pub items: Vec<TrayItem>,
}

pub fn clock(ms: i64) -> String {
    let t = (ms.max(0)) / 1000;
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn item(label: &str, action: Option<&'static str>, enabled: bool) -> TrayItem {
    TrayItem {
        label: label.into(),
        action,
        enabled,
        separator: false,
    }
}
fn sep() -> TrayItem {
    TrayItem {
        label: String::new(),
        action: None,
        enabled: false,
        separator: true,
    }
}

pub fn tray_state(v: &Value, now: i64) -> TrayState {
    let phase = v["phase"].as_str().unwrap_or("idle");
    let helper = v["helper"].as_str().unwrap_or("ok");
    let recording = matches!(phase, "starting" | "recording" | "reconnecting");
    let elapsed = match (recording, v["startedAt"].as_i64()) {
        (true, Some(s)) if s > 0 => now - s,
        _ => v["elapsedMs"].as_i64().unwrap_or(0),
    };
    let detected_title = v["detected"]["title"].as_str();
    let status = if helper == "missing" {
        UI_HELPER_MISSING.to_string()
    } else if recording {
        match detected_title {
            Some(t) if !t.is_empty() => format!("{UI_RECORDING} · {} · {t}", clock(elapsed)),
            _ => format!("{UI_RECORDING} · {}", clock(elapsed)),
        }
    } else if phase == "detected" {
        format!("{UI_MEETING_DETECTED} · {}", detected_title.unwrap_or(""))
            .trim()
            .to_string()
    } else if phase == "processing" {
        UI_SUMMARIZING.into()
    } else {
        UI_NOT_RECORDING.into()
    };
    let mut items = vec![item(&status, None, false), sep()];
    if recording {
        items.push(item(UI_STOP, Some("stop"), true));
    } else {
        let label = if phase == "detected" {
            UI_RECORD_DETECTED
        } else {
            UI_START
        };
        items.push(item(
            label,
            Some("start"),
            phase != "stopping" && helper != "missing",
        ));
    }
    if phase == "detected" {
        items.push(item(UI_NOT_NOW, Some("dismiss"), true));
    }
    items.extend([
        sep(),
        item(UI_OPEN, Some("open"), true),
        item(UI_QUIT, Some("quit"), true),
    ]);
    TrayState {
        recording,
        title: if recording { clock(elapsed) } else { String::new() },
        tooltip: format!("{UI_APP} · {status}"),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labels(s: &TrayState) -> Vec<String> {
        s.items
            .iter()
            .filter(|i| !i.separator)
            .map(|i| i.label.clone())
            .collect()
    }

    #[test]
    fn idle() {
        let s = tray_state(&json!({"phase": "idle", "helper": "ok", "elapsedMs": 0}), 0);
        assert!(!s.recording);
        assert_eq!(s.title, "");
        assert_eq!(
            labels(&s),
            ["not recording", "start recording", "open qrtz", "quit qrtz"]
        );
        assert_eq!(s.tooltip, "qrtz · not recording");
    }

    #[test]
    fn recording_shows_elapsed_and_stop() {
        let s = tray_state(
            &json!({"phase": "recording", "startedAt": 1_000, "detected": {"title": "standup"}, "helper": "ok"}),
            1_000 + 65_000,
        );
        assert!(s.recording);
        assert_eq!(s.title, "1:05");
        assert_eq!(labels(&s)[0], "recording · 1:05 · standup");
        assert_eq!(labels(&s)[1], "stop recording");
    }

    #[test]
    fn detected_offers_record_and_not_now() {
        let s = tray_state(&json!({"phase": "detected", "detected": {"title": "sync"}}), 0);
        assert_eq!(
            labels(&s)[..3],
            ["meeting detected · sync", "record this meeting", "not now"]
        );
    }

    #[test]
    fn helper_missing_disables_start() {
        let s = tray_state(&json!({"phase": "idle", "helper": "missing"}), 0);
        assert_eq!(labels(&s)[0], "recording unavailable (helper missing)");
        assert!(
            !s.items
                .iter()
                .find(|i| i.action == Some("start"))
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn clock_formats() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(3_725_000), "1:02:05");
        assert_eq!(clock(-5), "0:00");
    }

    #[test]
    fn all_strings_lowercase() {
        for s in [
            UI_APP,
            UI_HUD,
            UI_OPEN,
            UI_QUIT,
            UI_START,
            UI_RECORD_DETECTED,
            UI_STOP,
            UI_NOT_NOW,
            UI_NOT_RECORDING,
            UI_SUMMARIZING,
            UI_HELPER_MISSING,
            UI_MEETING_DETECTED,
            UI_RECORDING,
        ] {
            assert_eq!(s, s.to_lowercase());
        }
    }
}

// ---- Tauri wiring ----
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};

const TRAY_ID: &str = "qrtz";
const ICON_IDLE: &[u8] = include_bytes!("../icons/tray-idle@2x.png");
const ICON_REC: &[u8] = include_bytes!("../icons/tray-rec@2x.png");

/// The status row's handle, so the 1 s tick can update the elapsed clock without rebuilding the menu
/// (rebuilding would close an open menu on macOS).
#[derive(Default)]
pub struct TrayHandles(std::sync::Mutex<Option<MenuItem<Wry>>>);

pub fn is_recording(session: &Value) -> bool {
    matches!(
        session["phase"].as_str(),
        Some("starting" | "recording" | "reconnecting")
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn menu(app: &AppHandle, s: &TrayState) -> tauri::Result<(Menu<Wry>, Option<MenuItem<Wry>>)> {
    let m = Menu::new(app)?;
    let mut status = None;
    for (i, it) in s.items.iter().enumerate() {
        if it.separator {
            m.append(&PredefinedMenuItem::separator(app)?)?;
            continue;
        }
        let id = it.action.unwrap_or("status");
        let mi = MenuItem::with_id(
            app,
            id,
            &it.label,
            it.enabled && it.action.is_some(),
            None::<&str>,
        )?;
        if i == 0 {
            status = Some(mi.clone());
        }
        m.append(&mi)?;
    }
    Ok((m, status))
}

fn icon(recording: bool) -> tauri::Result<Image<'static>> {
    Image::from_bytes(if recording { ICON_REC } else { ICON_IDLE })
}

pub fn create(app: &AppHandle) -> tauri::Result<TrayIcon> {
    app.manage(TrayHandles::default());
    let s = tray_state(&json_idle(), now_ms());
    let (m, status) = menu(app, &s)?;
    *app.state::<TrayHandles>()
        .0
        .lock()
        .unwrap_or_else(crate::poison::recover) = status;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(false)?)
        .icon_as_template(true)
        .tooltip(&s.tooltip)
        .menu(&m)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, ev| crate::backend::tray_action(app, ev.id().as_ref()))
        .build(app)
}

fn json_idle() -> Value {
    serde_json::json!({"phase": "idle", "helper": "ok", "elapsedMs": 0})
}

/// Session changed: rebuild menu, icon, title and tooltip.
pub fn update(app: &AppHandle, session: &Value) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let s = tray_state(session, now_ms());
    if let Ok((m, status)) = menu(app, &s) {
        let _ = tray.set_menu(Some(m));
        *app.state::<TrayHandles>()
            .0
            .lock()
            .unwrap_or_else(crate::poison::recover) = status;
    }
    if let Ok(i) = icon(s.recording) {
        let _ = tray.set_icon(Some(i));
        let _ = tray.set_icon_as_template(true);
    }
    let _ = tray.set_title(Some(&s.title));
    let _ = tray.set_tooltip(Some(&s.tooltip));
}

/// Recording clock tick: title, tooltip and the status row only.
pub fn tick(app: &AppHandle, session: &Value) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let s = tray_state(session, now_ms());
    let _ = tray.set_title(Some(&s.title));
    let _ = tray.set_tooltip(Some(&s.tooltip));
    if let Some(mi) = app
        .state::<TrayHandles>()
        .0
        .lock()
        .unwrap_or_else(crate::poison::recover)
        .as_ref()
    {
        let _ = mi.set_text(&s.items[0].label);
    }
}

pub fn set_visible(app: &AppHandle, on: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_visible(on);
    }
}
