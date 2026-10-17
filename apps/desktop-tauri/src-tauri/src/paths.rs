//! Where things live: data dir and the helper binary.
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_helper_bridge::ProcessSpawn;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// QRTZ_USER_DATA (tests, side-by-side runs) wins over the platform app-data dir.
pub fn data_dir(app: &AppHandle) -> PathBuf {
    std::env::var_os("QRTZ_USER_DATA")
        .map(PathBuf::from)
        .or_else(|| app.path().app_data_dir().ok())
        .unwrap_or_else(|| std::env::temp_dir().join("qrtz"))
}

pub fn recordings_dir(app: &AppHandle) -> PathBuf {
    data_dir(app).join("recordings")
}

/// Env knobs forwarded to the mock helper (e2e fault injection; same names as the TS mock had).
const MOCK_ENV: &[&str] = &[
    "QRTZ_MOCK_SCENARIO",
    "QRTZ_MOCK_SPEED",
    "QRTZ_MOCK_PERMISSIONS",
    "QRTZ_MOCK_REVOKE",
    "QRTZ_MOCK_SILENT",
    "QRTZ_MOCK_MIC_FAILS",
    "QRTZ_MOCK_FALLBACK",
    "QRTZ_MOCK_INSERT",
    "QRTZ_MOCK_CRASH_AFTER_MS",
    "QRTZ_MOCK_HB_MS",
];

/// Same resolution order as the Electron helper-process.ts:
///   QRTZ_SIMULATE_MISSING_HELPER=1 → none; QRTZ_HELPER_BIN → bundled sidecar (Contents/MacOS/qrtz-helper) →
///   repo SwiftPM build (debug) → the Rust mock helper next to this binary (QRTZ_MOCK_HELPER=1, or non-macOS).
pub fn helper_spawn() -> Option<(ProcessSpawn, HelperAvailability)> {
    let dev = cfg!(debug_assertions) || std::env::var("QRTZ_ALLOW_DEV_KNOBS").as_deref() == Ok("1");
    if dev && std::env::var("QRTZ_SIMULATE_MISSING_HELPER").as_deref() == Ok("1") {
        return None;
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mock = exe_dir.join("qrtz-mock-helper");
    let mock_spawn = || {
        let mut sp = ProcessSpawn::helper(mock.clone());
        sp.env = MOCK_ENV
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        (sp, HelperAvailability::Simulated)
    };
    if std::env::var("QRTZ_MOCK_HELPER").as_deref() == Ok("1") && mock.exists() {
        return Some(mock_spawn());
    }
    if let Some(p) = std::env::var_os("QRTZ_HELPER_BIN") {
        return Some((ProcessSpawn::helper(PathBuf::from(p)), HelperAvailability::Native));
    }
    let bundled = exe_dir.join("qrtz-helper");
    if bundled.exists() {
        return Some((ProcessSpawn::helper(bundled), HelperAvailability::Native));
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../native/qrtz-helper/.build/release/qrtz-helper");
    if cfg!(debug_assertions) && repo.exists() {
        return Some((ProcessSpawn::helper(repo), HelperAvailability::Native));
    }
    if !cfg!(target_os = "macos") && mock.exists() {
        return Some(mock_spawn());
    }
    None
}
