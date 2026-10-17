fn main() {
    // App commands get generated `allow-<command>` permissions that capabilities/default.json grants.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["qrtz_invoke", "qrtz_subscribe"])),
    )
    .expect("tauri build script");
}
