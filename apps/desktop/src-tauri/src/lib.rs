//! qrtz Tauri 2 shell: windows, tray, global shortcuts, login item, notifications and the two IPC commands.
//! Everything behavioural lives in the UI-free crates (qrtz-app, qrtz-ipc, qrtz-helper-bridge, qrtz-lifecycle,
//! qrtz-storage); see docs/tauri-migration-status.md.
mod backend;
mod commands;
mod media;
mod paths;
mod policy;
pub mod strings;
mod tray;
mod windows;

use tauri::webview::PageLoadEvent;
use tauri::{Manager, RunEvent, WindowEvent};

pub fn run() {
    let started = std::time::Instant::now();
    // ADR 0083: stderr plus the local, size-capped log file (~/Library/Logs/qrtz/qrtz.log on macOS). A release
    // build reads only a bare level from QRTZ_LOG.
    let release = !cfg!(debug_assertions);
    qrtz_app::logging::init(qrtz_app::logging::LogOptions {
        dir: qrtz_app::logging::log_dir(
            std::env::consts::OS,
            qrtz_app::logging::home_dir().as_deref(),
            |k| std::env::var(k).ok(),
        ),
        release,
        env_filter: std::env::var("QRTZ_LOG").ok(),
        version: env!("CARGO_PKG_VERSION").into(),
        build: format!(
            "{} {} tauri {}",
            if release { "release" } else { "debug" },
            option_env!("QRTZ_BUILD_ID").unwrap_or("dev"),
            tauri::VERSION
        ),
    });

    let mut builder = tauri::Builder::default();
    // A second launch focuses the running app (Electron requestSingleInstanceLock). Off in e2e, where several
    // isolated instances run side by side.
    if std::env::var("QRTZ_MULTI_INSTANCE").as_deref() != Ok("1") {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            windows::show_main(app)
        }));
    }
    let app = builder
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("qrtz-media", |ctx, req, responder| {
            let dir = paths::recordings_dir(ctx.app_handle());
            std::thread::spawn(move || responder.respond(media::serve(&dir, &req)));
        })
        .invoke_handler(tauri::generate_handler![
            commands::qrtz_invoke,
            commands::qrtz_subscribe
        ])
        .on_page_load(|webview, payload| {
            // A reload starts a fresh renderer that re-subscribes; drop the old page's subscriptions.
            if payload.event() == PageLoadEvent::Started {
                backend::forget_webview(webview.app_handle(), webview.label());
            }
        })
        .on_window_event(|window, ev| {
            if let WindowEvent::Destroyed = ev {
                backend::forget_webview(window.app_handle(), window.label());
            }
        })
        .setup(move |app| backend::setup(app, started))
        .build(tauri::generate_context!())
        .expect("error while building qrtz");

    app.run(|app, ev| match ev {
        // Closing the last window keeps qrtz running on macOS (dock + menu bar); elsewhere it quits, like Electron.
        RunEvent::ExitRequested { api, code: None, .. } if cfg!(target_os = "macos") => api.prevent_exit(),
        RunEvent::Exit => backend::shutdown(app),
        _ => {}
    });
}
