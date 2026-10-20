//! The two Tauri commands behind `createTauriTransport` (packages/core/src/ipc/tauri-transport.ts).
//! Everything else (envelope validation, capability checks, routing) lives in qrtz-ipc / qrtz-app.
use crate::backend::Backend;
use qrtz_ipc::{AppError, ErrorCode};
use serde_json::{json, Value};
use tauri::{Manager, Webview};

/// Only our bundled renderer may talk to the backend: tauri:// (macOS/Linux), http(s)://tauri.localhost
/// (Windows), or the Vite dev server in debug builds. Mirrors `isTrustedSender` in apps/desktop.
pub fn trusted_origin(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    match scheme {
        "tauri" => host == "localhost",
        "http" | "https" => host == "tauri.localhost" || (cfg!(debug_assertions) && host == "localhost:1420"),
        _ => false,
    }
}

fn sender_ok(webview: &Webview) -> bool {
    webview.url().map(|u| trusted_origin(u.as_str())).unwrap_or(false)
}

fn forbidden() -> AppError {
    AppError::new(ErrorCode::Forbidden, "untrusted sender")
}

#[tauri::command]
pub async fn qrtz_invoke(envelope: Value, webview: Webview) -> Result<Value, String> {
    if !sender_ok(&webview) {
        let id = envelope["id"].as_str().unwrap_or("").to_owned();
        return Ok(json!({"ok": false, "id": id, "error": forbidden().to_json()}));
    }
    let backend = webview.state::<Backend>();
    Ok(backend.router.dispatch(envelope).await)
}

#[tauri::command]
pub fn qrtz_subscribe(sub_app: String, event: String, webview: Webview) -> Value {
    if !sender_ok(&webview) {
        return json!({"ok": false, "error": forbidden().to_json()});
    }
    let backend = webview.state::<Backend>();
    let r = backend
        .subs
        .lock()
        .unwrap_or_else(crate::poison::recover)
        .subscribe(webview.label(), &sub_app, &event);
    if webview.label() == crate::windows::MAIN {
        backend.mark_ready();
    }
    match r {
        Ok(()) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e.to_json()}),
    }
}

/// ADR 0095: open this window's stream for a high-rate event, delivered on `channel`. Same capability rule as
/// `qrtz_subscribe`; a window holds one stream per event (other sub apps join it).
#[tauri::command]
pub fn qrtz_stream_open(
    sub_app: String,
    event: String,
    channel: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
    webview: Webview,
) -> Value {
    if !sender_ok(&webview) {
        return json!({"ok": false, "error": forbidden().to_json()});
    }
    let backend = webview.state::<Backend>();
    let r = backend
        .streams
        .lock()
        .unwrap_or_else(crate::poison::recover)
        .open(webview.label(), &sub_app, &event, channel);
    if webview.label() == crate::windows::MAIN {
        backend.mark_ready();
    }
    match r {
        Ok(id) => json!({"ok": true, "id": id}),
        Err(e) => json!({"ok": false, "error": e.to_json()}),
    }
}

/// Another sub app in the same window listens on stream `id`.
#[tauri::command]
pub fn qrtz_stream_join(id: u64, sub_app: String, webview: Webview) -> Value {
    if !sender_ok(&webview) {
        return json!({"ok": false, "error": forbidden().to_json()});
    }
    let r = webview
        .state::<Backend>()
        .streams
        .lock()
        .unwrap_or_else(crate::poison::recover)
        .join(webview.label(), id, &sub_app);
    match r {
        Ok(()) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e.to_json()}),
    }
}

/// The window's last listener on stream `id` went away.
#[tauri::command]
pub fn qrtz_stream_close(id: u64, webview: Webview) -> Value {
    if !sender_ok(&webview) {
        return json!({"ok": false, "error": forbidden().to_json()});
    }
    let closed = webview
        .state::<Backend>()
        .streams
        .lock()
        .unwrap_or_else(crate::poison::recover)
        .close(webview.label(), id);
    json!({"ok": closed})
}

#[cfg(test)]
mod tests {
    use super::trusted_origin;

    #[test]
    fn only_the_bundled_renderer_is_trusted() {
        assert!(trusted_origin("tauri://localhost/hud.html"));
        assert!(trusted_origin("http://tauri.localhost/index.html"));
        assert!(trusted_origin("https://tauri.localhost/"));
        assert_eq!(trusted_origin("http://localhost:1420/"), cfg!(debug_assertions));
        for bad in [
            "https://evil.example/",
            "http://tauri.localhost.evil.example/",
            "tauri://evil/",
            "file:///index.html",
            "http://localhost:1421/",
            "garbage",
        ] {
            assert!(!trusted_origin(bad), "{bad}");
        }
    }
}
