fn main() {
    // App commands get generated `allow-<command>` permissions that capabilities/default.json grants.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "qrtz_invoke",
            "qrtz_subscribe",
            "qrtz_stream_open",
            "qrtz_stream_join",
            "qrtz_stream_close",
        ]),
    ))
    .expect("tauri build script");
}
