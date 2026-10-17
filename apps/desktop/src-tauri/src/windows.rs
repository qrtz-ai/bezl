//! Window factory: main (index.html), HUD (hud.html) and talk pill (pill.html). Each window has its own page
//! and entry, so the HUD and the pill never load the shell, its router or the sub apps (ADR 0090).
use crate::strings::{UI_APP, UI_HUD, UI_PILL};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

pub const MAIN: &str = "main";
pub const HUD: &str = "hud";
pub const PILL: &str = "pill";
pub const HUD_W: f64 = 340.0;
pub const HUD_H: f64 = 56.0;
pub const HUD_EXPANDED_H: f64 = 220.0;
const PILL_W: f64 = 240.0;
const PILL_H: f64 = 52.0;

fn url(view: &str) -> WebviewUrl {
    WebviewUrl::App(if view.is_empty() {
        "index.html".into()
    } else {
        format!("{view}.html").into()
    })
}

/// (x, y, w, h) of the primary monitor's work area in logical pixels.
fn work_area(app: &AppHandle) -> (f64, f64, f64, f64) {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let s = m.scale_factor();
            let a = m.work_area();
            (
                a.position.x as f64 / s,
                a.position.y as f64 / s,
                a.size.width as f64 / s,
                a.size.height as f64 / s,
            )
        })
        .unwrap_or((0.0, 0.0, 1440.0, 900.0))
}

pub fn create_main(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let b = WebviewWindowBuilder::new(app, MAIN, url(""))
        .title(UI_APP)
        .inner_size(1180.0, 760.0)
        .min_inner_size(860.0, 560.0)
        .visible(std::env::var("QRTZ_HIDDEN").as_deref() != Ok("1"));
    #[cfg(target_os = "macos")]
    let b = b
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    b.build()
}

pub fn show_main(app: &AppHandle) {
    match app.get_webview_window(MAIN) {
        Some(w) => {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
        None => {
            if let Err(e) = create_main(app) {
                tracing::error!(err = %e, "could not create main window");
            }
        }
    }
}

pub fn show_hud(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(HUD) {
        let _ = w.show();
        return;
    }
    let (x, y, w, _) = work_area(app);
    let r = WebviewWindowBuilder::new(app, HUD, url("hud"))
        .title(UI_HUD)
        .inner_size(HUD_W, HUD_H)
        .position(x + w - HUD_W - 16.0, y + 16.0)
        .decorations(false)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .focused(false)
        .background_color(tauri::window::Color(0x11, 0x11, 0x11, 0xff))
        .build();
    if let Err(e) = r {
        tracing::error!(err = %e, "could not create hud");
    }
}

pub fn close(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.close();
    }
}

pub fn hide(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.hide();
    }
}

pub fn set_hud_expanded(app: &AppHandle, expanded: bool) -> bool {
    let Some(w) = app.get_webview_window(HUD) else {
        return expanded;
    };
    let _ = w.set_size(LogicalSize::new(
        HUD_W,
        if expanded { HUD_EXPANDED_H } else { HUD_H },
    ));
    if expanded {
        let _ = w.set_focus();
    }
    expanded
}

pub fn show_pill(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PILL) {
        let _ = w.show();
        return;
    }
    let (x, y, w, h) = work_area(app);
    let b = WebviewWindowBuilder::new(app, PILL, url("pill"))
        .title(UI_PILL)
        .inner_size(PILL_W, PILL_H)
        .position(x + ((w - PILL_W) / 2.0).round(), y + h - PILL_H - 24.0)
        .decorations(false)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .focused(false)
        .shadow(false);
    #[cfg(not(target_os = "linux"))]
    let b = b.transparent(true);
    match b.build() {
        Ok(win) => {
            let _ = win.set_ignore_cursor_events(true);
            let _ = win.set_position(LogicalPosition::new(
                x + ((w - PILL_W) / 2.0).round(),
                y + h - PILL_H - 24.0,
            ));
        }
        Err(e) => tracing::error!(err = %e, "could not create pill"),
    }
}

#[cfg(test)]
mod tests {
    use super::url;
    use std::path::PathBuf;
    use tauri::WebviewUrl;

    fn page(view: &str) -> PathBuf {
        match url(view) {
            WebviewUrl::App(p) => p,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_window_loads_its_own_page() {
        assert_eq!(page(""), PathBuf::from("index.html"));
        assert_eq!(page(super::HUD), PathBuf::from("hud.html"));
        assert_eq!(page(super::PILL), PathBuf::from("pill.html"));
    }
}
