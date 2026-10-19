//! Where things live: data dir and the helper binary.
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_helper_bridge::ProcessSpawn;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// The platform app-data dir; in a debug build `QRTZ_USER_DATA` (tests, side-by-side runs) wins over it.
pub fn data_dir(app: &AppHandle) -> PathBuf {
    data_dir_from(
        USER_DATA_KNOB,
        |k| std::env::var_os(k),
        app.path().app_data_dir().ok(),
    )
}

/// Whether this build takes its data dir from `QRTZ_USER_DATA`: a debug build only, decided at compile time
/// (Bozniak's 902bcab review, design call (b)). The data dir holds `qrtz.lock` and `packages/`, which say what code
/// runs with qrtz's TCC grants, so a release build never lets the environment pick it: a planted dir could
/// otherwise run an old, revoked (but still Developer ID signed) module. The `qrtz` CLI keeps the variable.
pub const USER_DATA_KNOB: bool = cfg!(debug_assertions);

fn data_dir_from(
    knob: bool,
    get: impl Fn(&str) -> Option<std::ffi::OsString>,
    platform: Option<PathBuf>,
) -> PathBuf {
    knob.then(|| get("QRTZ_USER_DATA"))
        .flatten()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or(platform)
        .unwrap_or_else(|| std::env::temp_dir().join("qrtz"))
}

/// The environment the log folder is chosen from: a release build hides `QRTZ_USER_DATA` (see [`USER_DATA_KNOB`]),
/// so its logs stay in the platform's log folder, next to the data dir it really uses.
pub fn log_env(k: &str) -> Option<String> {
    if k == "QRTZ_USER_DATA" && !USER_DATA_KNOB {
        return None;
    }
    std::env::var(k).ok()
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
///   module counts as installed); QRTZ_HELPER_BIN → that monolith (a debug build only: where code comes from is
///   never a runtime knob, so not even QRTZ_ALLOW_DEV_KNOBS=1 lets a release build run it; Bozniak, 902bcab review);
///   then macOS → the installed modules; elsewhere → the mock helper if present.
pub fn helper_spawn() -> Option<(ProcessSpawn, HelperAvailability)> {
    let dev = cfg!(debug_assertions) || std::env::var("QRTZ_ALLOW_DEV_KNOBS").as_deref() == Ok("1");
    let code_knobs = CODE_KNOBS;
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
    if code_knobs {
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

/// Whether this build may take a helper executable's path from the environment (`QRTZ_HELPER_BIN`,
/// `QRTZ_<ID>_BIN`): a debug build only, decided at compile time. ADR 0154 gate (h): whatever qrtz spawns inherits
/// its TCC grants (microphone, Accessibility), so in a release build a path from the environment would hand those
/// grants to any binary. `QRTZ_ALLOW_DEV_KNOBS=1` (the E2E harness's test knobs) never turns this on.
pub const CODE_KNOBS: bool = cfg!(debug_assertions);

/// ADR 0169: every module helper from its installed package under `data`. `QRTZ_<ID>_BIN` still overrides one in a
/// debug build; a release build never takes a path from the environment (see [`CODE_KNOBS`]).
pub fn installed_layout(data: &std::path::Path) -> qrtz_app::module_helpers::HelperLayout {
    // ADR 0174: echo ships in qrtz.app next to the app binary (Contents/MacOS/qrtz-echo).
    // Resolved through any symlink (macOS reports the path the app was exec'd by); a release build only takes a dir
    // that is really `<something>.app/Contents/MacOS`, else bundled echo has no path and never starts.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .filter(|d| CODE_KNOBS || is_app_macos_dir(d))
        .unwrap_or_default();
    installed_layout_from(data, &exe_dir, CODE_KNOBS, |k| std::env::var(k).ok())
}

/// `dir` is `<name>.app/Contents/MacOS` (ADR 0174: where qrtz-echo is bundled).
fn is_app_macos_dir(dir: &std::path::Path) -> bool {
    let mut c = dir.components().rev();
    matches!(
        (c.next(), c.next(), c.next()),
        (Some(m), Some(k), Some(app))
            if m.as_os_str() == "MacOS"
                && k.as_os_str() == "Contents"
                && app.as_os_str().to_string_lossy().ends_with(".app")
    )
}

fn installed_layout_from(
    data: &std::path::Path,
    exe_dir: &std::path::Path,
    dev: bool,
    get: impl Fn(&str) -> Option<String>,
) -> qrtz_app::module_helpers::HelperLayout {
    // Only dev-local (debug + the `dev-local` feature, checked by qrtz_app::dev_local::activate) may name a module's
    // program; the environment alone never does.
    let _ = get;
    let programs = if dev && qrtz_app::dev_local::ENABLED {
        qrtz_app::dev_local::programs()
    } else {
        Default::default()
    };
    qrtz_app::module_helpers::HelperLayout::Installed {
        root: data.to_path_buf(),
        programs,
        bundled: qrtz_app::module_helpers::BUNDLED_EXTENSIONS
            .iter()
            .map(|id| ((*id).to_owned(), exe_dir.join(format!("qrtz-{id}"))))
            .collect(),
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
/// off). Where a module app is (`QRTZ_<ID>_BIN`) may only be overridden in a debug build ([`CODE_KNOBS`]); a release
/// build runs the apps next to the helper.
pub fn helper_layout() -> qrtz_app::module_helpers::HelperLayout {
    layout_from(CODE_KNOBS, |k| std::env::var(k).ok())
}

fn layout_from(dev: bool, get: impl Fn(&str) -> Option<String>) -> qrtz_app::module_helpers::HelperLayout {
    qrtz_app::module_helpers::HelperLayout::from_vars(
        |k| if k.ends_with("_BIN") && !dev { None } else { get(k) },
    )
}

#[cfg(test)]
mod layout_tests {
    #[test]
    fn bundled_echo_only_from_an_app_macos_dir() {
        use std::path::Path;
        assert!(super::is_app_macos_dir(Path::new(
            "/Applications/qrtz.app/Contents/MacOS"
        )));
        for d in [
            "/tmp",
            "/tmp/MacOS",
            "/x/Contents/MacOS",
            "/x.app/MacOS",
            "/x.app/Contents/MacOS/..",
        ] {
            assert!(!super::is_app_macos_dir(Path::new(d)), "{d}");
        }
    }

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
            super::installed_layout_from(data, std::path::Path::new("/app/MacOS"), false, get),
            HelperLayout::Installed {
                root: data.into(),
                programs: Default::default(),
                bundled: [("echo".to_owned(), "/app/MacOS/qrtz-echo".into())].into(),
            }
        );
        let HelperLayout::Installed { programs, .. } =
            super::installed_layout_from(data, std::path::Path::new("/app/MacOS"), true, get)
        else {
            panic!()
        };
        // dev-local's gate: the environment alone names no program, even in a debug build (nothing activated here).
        assert!(programs.is_empty());
    }

    /// Design call (b): a release build's data dir is the platform's, whatever `QRTZ_USER_DATA` says; a debug
    /// build (tests, side-by-side runs) takes the variable.
    #[test]
    fn only_a_debug_build_takes_the_data_dir_from_the_environment() {
        use std::path::PathBuf;
        let get = |k: &str| (k == "QRTZ_USER_DATA").then(|| std::ffi::OsString::from("/tmp/planted"));
        let app_data = PathBuf::from("/Users/j/Library/Application Support/ai.qrtz.desktop");
        let platform = || Some(app_data.clone());
        assert_eq!(super::data_dir_from(false, get, platform()), app_data);
        assert_eq!(
            super::data_dir_from(true, get, platform()),
            PathBuf::from("/tmp/planted")
        );
        assert_eq!(
            super::data_dir_from(true, |_| Some(std::ffi::OsString::new()), platform()),
            app_data,
            "an empty QRTZ_USER_DATA is unset"
        );
        let src = include_str!("paths.rs");
        let src = &src[..src.find("#[cfg(test)]").expect("tests")];
        assert!(src.contains("pub const USER_DATA_KNOB: bool = cfg!(debug_assertions);"));
        let body = &src[src.find("pub fn data_dir(").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        assert!(
            body.contains("USER_DATA_KNOB") && !body.contains("QRTZ_ALLOW_DEV_KNOBS"),
            "{body}"
        );
        if !cfg!(debug_assertions) {
            assert!(!super::USER_DATA_KNOB);
            assert_eq!(super::log_env("QRTZ_USER_DATA"), None);
        }
    }

    /// 902bcab review (Bozniak): where helper code comes from follows `cfg!(debug_assertions)` alone. A guard
    /// against wiring `QRTZ_ALLOW_DEV_KNOBS` back into a path that picks an executable: that switch is a runtime
    /// env var, and a release qrtz started with it would spawn any binary with qrtz's TCC grants (gate h).
    #[test]
    fn no_runtime_switch_picks_helper_code() {
        let src = include_str!("paths.rs");
        let src = &src[..src.find("#[cfg(test)]").expect("tests")];
        assert!(src.contains("pub const CODE_KNOBS: bool = cfg!(debug_assertions);"));
        for f in ["pub fn installed_layout(", "pub fn helper_layout("] {
            let body = &src[src.find(f).expect(f)..];
            let body = &body[..body.find("\n}\n").expect("end of fn")];
            assert!(body.contains("CODE_KNOBS"), "{f}");
            assert!(!body.contains("QRTZ_ALLOW_DEV_KNOBS"), "{f}");
        }
        let spawn = &src[src.find("pub fn helper_spawn(").unwrap()..];
        let spawn = &spawn[..spawn.find("\n}\n").unwrap()];
        assert!(
            spawn.contains("if code_knobs {\n        if let Some(p) = std::env::var_os(\"QRTZ_HELPER_BIN\")"),
            "QRTZ_HELPER_BIN behind CODE_KNOBS"
        );
        assert_eq!(spawn.matches("QRTZ_HELPER_BIN\")").count(), 1);
        if !cfg!(debug_assertions) {
            assert!(
                !super::CODE_KNOBS,
                "a release build takes no helper path from the environment"
            );
        }
    }
}
