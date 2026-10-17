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
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("QRTZ_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();

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
