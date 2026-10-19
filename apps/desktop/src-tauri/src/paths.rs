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

/// Where the helpers come from. ADR 0169: qrtz.app bundles none, so on macOS the answer is the installed modules
/// ([`qrtz_app::module_helpers::INSTALLED_PROGRAM`]: each module's helper is its package's entry, see
/// `installed_layout`). Dev and e2e knobs first:
///   QRTZ_SIMULATE_MISSING_HELPER=1 → none; QRTZ_MOCK_HELPER=1 → the Rust mock helper next to this binary (every
///   module counts as installed); QRTZ_HELPER_BIN → that monolith (a debug build or QRTZ_ALLOW_DEV_KNOBS=1 only);
///   then macOS → the installed modules; elsewhere → the mock helper if present.
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
    if dev {
        if let Some(p) = std::env::var_os("QRTZ_HELPER_BIN") {
            return Some((ProcessSpawn::helper(PathBuf::from(p)), HelperAvailability::Native));
        }
    }
    if cfg!(target_os = "macos") {
        return Some((
            ProcessSpawn::helper(PathBuf::from(qrtz_app::module_helpers::INSTALLED_PROGRAM)),
            HelperAvailability::Native,
        ));
    }
    if mock.exists() {
        return Some(mock_spawn());
    }
    None
}

/// True when the helpers are the installed modules (ADR 0169): qrtz then tracks `qrtz.lock` and starts empty.
pub fn is_installed_modules(spawn: &ProcessSpawn) -> bool {
    spawn.program.as_os_str() == qrtz_app::module_helpers::INSTALLED_PROGRAM
}

/// ADR 0169: every module helper from its installed package under `data`. `QRTZ_<ID>_BIN` still overrides one in a
/// dev build (or under `QRTZ_ALLOW_DEV_KNOBS=1`); a release build never takes a path from the environment.
pub fn installed_layout(data: &std::path::Path) -> qrtz_app::module_helpers::HelperLayout {
    let dev = cfg!(debug_assertions) || std::env::var("QRTZ_ALLOW_DEV_KNOBS").as_deref() == Ok("1");
    installed_layout_from(data, dev, |k| std::env::var(k).ok())
}

fn installed_layout_from(
    data: &std::path::Path,
    dev: bool,
    get: impl Fn(&str) -> Option<String>,
) -> qrtz_app::module_helpers::HelperLayout {
    let programs = if dev {
        ["nano", "echo", "ctrl", "snap"]
            .iter()
            .filter_map(|id| {
                get(&format!("QRTZ_{}_BIN", id.to_uppercase()))
                    .filter(|p| !p.is_empty())
                    .map(|p| ((*id).to_owned(), PathBuf::from(p)))
            })
            .collect()
    } else {
        Default::default()
    };
    qrtz_app::module_helpers::HelperLayout::Installed {
        root: data.to_path_buf(),
        programs,
    }
}

/// Show `dir` in the file manager (Finder selects `qrtz.log` if it exists). The program is fixed and the argument
/// is a local path, so this can't open a URL. On unix the spawn takes the N98 fork guard, like every qrtz child.
pub fn reveal(dir: &std::path::Path) -> bool {
    use std::process::{Command, Stdio};
    let log = dir.join(qrtz_app::logging::LOG_FILE);
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("/usr/bin/open");
        if log.is_file() {
            c.arg("-R").arg(&log);
        } else {
            c.arg(dir);
        }
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        c.arg(dir);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(dir);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    let child = qrtz_cloexec::spawn(|| cmd.spawn());
    #[cfg(not(unix))]
    let child = cmd.spawn();
    match child.and_then(|mut c| c.wait()) {
        Ok(st) => st.success(),
        Err(e) => {
            tracing::warn!(kind = %e.kind(), "couldn't open the log folder");
            false
        }
    }
}

/// ADR 0146 M2.4: `QRTZ_MODULE_HELPERS=split` runs qrtz-nano, qrtz-echo and qrtz-ctrl instead of qrtz-helper (default
/// off). Where a module app is (`QRTZ_<ID>_BIN`) may only be overridden in a dev build or under
/// `QRTZ_ALLOW_DEV_KNOBS=1`; a release build runs the bundled apps next to the helper.
pub fn helper_layout() -> qrtz_app::module_helpers::HelperLayout {
    let dev = cfg!(debug_assertions) || std::env::var("QRTZ_ALLOW_DEV_KNOBS").as_deref() == Ok("1");
    layout_from(dev, |k| std::env::var(k).ok())
}

fn layout_from(dev: bool, get: impl Fn(&str) -> Option<String>) -> qrtz_app::module_helpers::HelperLayout {
    qrtz_app::module_helpers::HelperLayout::from_vars(
        |k| if k.ends_with("_BIN") && !dev { None } else { get(k) },
    )
}

#[cfg(test)]
mod layout_tests {
    use super::layout_from;
    use qrtz_app::module_helpers::HelperLayout;

    #[test]
    fn a_release_build_ignores_module_app_overrides() {
        let get = |k: &str| match k {
            "QRTZ_MODULE_HELPERS" => Some("split".to_owned()),
            "QRTZ_ECHO_BIN" => Some("/tmp/evil".to_owned()),
            _ => None,
        };
        assert_eq!(
            layout_from(false, get),
            HelperLayout::Split {
                programs: Default::default()
            }
        );
        let HelperLayout::Split { programs } = layout_from(true, get) else {
            panic!()
        };
        assert_eq!(programs.len(), 1);
        assert_eq!(layout_from(true, |_| None), HelperLayout::Monolith);
    }

    #[test]
    fn the_installed_layout_takes_no_path_from_a_release_environment() {
        let get = |k: &str| (k == "QRTZ_SNAP_BIN").then(|| "/tmp/evil".to_owned());
        let data = std::path::Path::new("/data");
        assert_eq!(
            super::installed_layout_from(data, false, get),
            HelperLayout::Installed {
                root: data.into(),
                programs: Default::default()
            }
        );
        let HelperLayout::Installed { programs, .. } = super::installed_layout_from(data, true, get) else {
            panic!()
        };
        assert_eq!(programs.len(), 1);
    }
}
