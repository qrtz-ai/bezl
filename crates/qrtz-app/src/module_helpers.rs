//! ADR 0146 M2.4: one helper process per module. With `QRTZ_MODULE_HELPERS=split` the host runs qrtz-nano
//! (recordings, meetings, summaries), qrtz-echo (dictation, the push key, insertion) and qrtz-ctrl (`ctrl.*`), each
//! under its own supervisor, instead of the one monolith qrtz-helper. The default stays the monolith (all three
//! roles share one supervisor) until the Mac has checked the split (the extension helpers running their controllers).
//!
//! Extensions are not apps (ADR 0146 M2.5): each helper is a plain executable the host spawns itself (fork/exec,
//! never LaunchServices), so qrtz stays its responsible process and every extension uses qrtz's own TCC grants:
//! one qrtz entry in Privacy settings, and echo's speaker gate uses qrtz's screen & system audio grant.
//!
//! The split layout runs each extension helper next to the helper binary (`qrtz-<id>`; `QRTZ_<ID>_BIN` overrides one).
//! It passes `QRTZ_MODULE=<id>`, and every process gets its own speech socket at fd 3 (`QRTZ_SPEECH_FD`): nano's
//! carries recording audio, echo's carries dictation audio. Lines are routed by the process they came from (see
//! `AppService::on_helper_line_from`).
use qrtz_helper_bridge::{HelperState, HelperStatus, ProcessSpawn, Spawn, Supervisor, SupervisorOptions};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// `split` turns the per-module helpers on. Anything else, or unset, keeps the monolith. dev-local builds only
/// (Bozniak, 23d892af): a release or plain debug build has neither this name nor the `QRTZ_<ID>_BIN` reads.
#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
pub const LAYOUT_ENV: &str = "QRTZ_MODULE_HELPERS";
/// Told to each module helper: which module it runs as.
pub const MODULE_ENV: &str = "QRTZ_MODULE";
/// The module helpers, in the order their status is reported.
pub const MODULE_HELPER_IDS: [&str; 3] = ["nano", "echo", "ctrl"];

/// Which helper processes the host runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HelperLayout {
    /// One qrtz-helper for echo, nano and ctrl (today's app).
    #[default]
    Monolith,
    /// qrtz-nano, qrtz-echo and qrtz-ctrl. `programs` overrides where a module's app is (`QRTZ_<ID>_BIN`).
    Split { programs: BTreeMap<String, PathBuf> },
    /// ADR 0169, the shipping app: qrtz.app bundles no helper. Each module's helper is its installed package's
    /// entry under `root` (`<data>/packages/<name>/<version>/`, read from `qrtz.lock` at every start), and only an
    /// installed, switched-on module's helper runs. `programs` are dev overrides (debug builds only).
    ///
    /// ADR 0174: `bundled` names the modules qrtz.app carries itself (echo → `Contents/MacOS/qrtz-echo`): their
    /// helper is that file, checked against the bundled manifest's requirement, with no lock or index needed.
    Installed {
        root: PathBuf,
        programs: BTreeMap<String, PathBuf>,
        bundled: BTreeMap<String, PathBuf>,
    },
}

impl HelperLayout {
    /// The split layout from the environment, unchecked: only [`crate::dev_local::split_layout`] calls this, after
    /// which every path goes through `check_bin`.
    #[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        if get(LAYOUT_ENV).as_deref().map(str::trim) != Some("split") {
            return Self::Monolith;
        }
        let programs = MODULE_HELPER_IDS
            .iter()
            .filter_map(|id| {
                get(&format!("QRTZ_{}_BIN", id.to_uppercase()))
                    .filter(|p| !p.is_empty())
                    .map(|p| ((*id).to_owned(), PathBuf::from(p)))
            })
            .collect();
        Self::Split { programs }
    }

    pub fn is_split(&self) -> bool {
        matches!(self, Self::Split { .. } | Self::Installed { .. })
    }

    /// Module `id`'s helper: next to the base helper (split), or its installed package (installed).
    pub fn spawner(&self, base: &ProcessSpawn, id: &str) -> Arc<dyn Spawn> {
        match self {
            Self::Monolith => Arc::new(module_spawn(base, id, &BTreeMap::new())),
            Self::Split { programs } => Arc::new(module_spawn(base, id, programs)),
            Self::Installed {
                root,
                programs,
                bundled,
            } => {
                let mut b = base.clone();
                b.env.push((MODULE_ENV.to_owned(), id.to_owned()));
                Arc::new(crate::installed::InstalledSpawn {
                    module: id.to_owned(),
                    root: root.clone(),
                    base: b,
                    fixed: programs.get(id).cloned(),
                    bundled: bundled
                        .get(id)
                        .filter(|_| qrtz_modules::is_bundled_extension(id))
                        .cloned(),
                    check: crate::installed::LaunchCheck::for_build(),
                })
            }
        }
    }
}

/// The placeholder program of the base spawn when the helpers come from installed packages (ADR 0169): never run
/// itself, it only carries the arguments and environment every module helper gets.
pub const INSTALLED_PROGRAM: &str = "qrtz-installed-module";

/// ADR 0174: the modules qrtz.app carries in `Contents/MacOS/qrtz-<id>` (v1: echo).
pub use qrtz_modules::BUNDLED_EXTENSIONS;

/// Module `id`'s app next to the helper binary. The Rust mock helper plays every role itself (Linux dev and tests),
/// told which by `QRTZ_MODULE`.
fn sibling(helper: &Path, id: &str) -> PathBuf {
    if helper.file_name().and_then(|n| n.to_str()) == Some("qrtz-mock-helper") {
        return helper.to_path_buf();
    }
    helper.with_file_name(format!("qrtz-{id}"))
}

/// The spawn for module `id`'s helper: same arguments and environment as the monolith's, plus `QRTZ_MODULE`.
pub fn module_spawn(base: &ProcessSpawn, id: &str, programs: &BTreeMap<String, PathBuf>) -> ProcessSpawn {
    let program = programs
        .get(id)
        .cloned()
        .unwrap_or_else(|| sibling(&base.program, id));
    let mut env = base.env.clone();
    env.push((MODULE_ENV.to_owned(), id.to_owned()));
    ProcessSpawn {
        program,
        args: base.args.clone(),
        env,
    }
}

/// Where a helper line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperSource {
    /// The monolith: every role.
    Monolith,
    Nano,
    Echo,
    Ctrl,
}

impl HelperSource {
    /// Lines for qrtz talk (echo).
    pub fn serves_echo(self) -> bool {
        matches!(self, Self::Monolith | Self::Echo)
    }
    /// Lines for the recording runtime (nano).
    pub fn serves_nano(self) -> bool {
        matches!(self, Self::Monolith | Self::Nano)
    }
}

/// The helper(s) each module talks to. In the monolith layout all three are the same supervisor.
#[derive(Clone)]
pub struct ModuleHelpers {
    pub nano: Supervisor,
    pub echo: Supervisor,
    pub ctrl: Supervisor,
    split: bool,
}

impl ModuleHelpers {
    /// Spawn the supervisors (not started: `AppService::start_helper` starts them). Call inside tokio.
    pub fn spawn(layout: &HelperLayout, base: ProcessSpawn, opts: SupervisorOptions) -> Self {
        match layout {
            HelperLayout::Monolith => Self::monolith(Supervisor::spawn(opts, Arc::new(base))),
            HelperLayout::Split { .. } | HelperLayout::Installed { .. } => {
                let mk = |id: &str| {
                    let mut o = opts.clone();
                    o.name = format!("qrtz-{id}");
                    // qrtz-ctrl speaks only `ctrl.*` (no heartbeat line), so its watchdog is off.
                    if id == "ctrl" {
                        o.heartbeat_timeout = Duration::ZERO;
                    }
                    Supervisor::spawn(o, layout.spawner(&base, id))
                };
                Self {
                    nano: mk("nano"),
                    echo: mk("echo"),
                    ctrl: mk("ctrl"),
                    split: true,
                }
            }
        }
    }

    pub fn monolith(s: Supervisor) -> Self {
        Self {
            nano: s.clone(),
            echo: s.clone(),
            ctrl: s,
            split: false,
        }
    }

    pub fn is_split(&self) -> bool {
        self.split
    }

    /// Each process once, with the lines' source.
    pub fn processes(&self) -> Vec<(HelperSource, Supervisor)> {
        if self.split {
            vec![
                (HelperSource::Nano, self.nano.clone()),
                (HelperSource::Echo, self.echo.clone()),
                (HelperSource::Ctrl, self.ctrl.clone()),
            ]
        } else {
            vec![(HelperSource::Monolith, self.nano.clone())]
        }
    }

    /// The `helper:status` the UI sees (and readiness): the monolith's, or nano's and echo's together. Run 31:
    /// qrtz-ctrl is left out, so a missing, crash-looping or switched-off ctrl never keeps echo and nano from
    /// counting as running; ctrl's own state reaches ctrl's runs through its wire (`helper.not_running`).
    pub fn status(&self) -> HelperStatus {
        if !self.split {
            return self.nano.status();
        }
        aggregate(&[self.nano.status(), self.echo.status()])
    }

    /// Graceful quit of every process at once (each gets `goodbye`, then `grace` before a kill).
    pub async fn shutdown(&self, goodbye: String, grace: Duration) {
        let mut set = tokio::task::JoinSet::new();
        for (_, s) in self.processes() {
            let g = goodbye.clone();
            set.spawn(async move { s.shutdown(g, grace).await });
        }
        while set.join_next().await.is_some() {}
    }
}

/// Several helpers as one status: running only when all are; otherwise the first (nano, echo) that isn't,
/// with that one's error. Restarts add up.
pub fn aggregate(all: &[HelperStatus]) -> HelperStatus {
    let restarts = all.iter().map(|s| s.restarts).sum();
    match all.iter().find(|s| s.state != HelperState::Running) {
        None => HelperStatus {
            state: if all.is_empty() {
                HelperState::Stopped
            } else {
                HelperState::Running
            },
            restarts,
            last_error: all.iter().find_map(|s| s.last_error.clone()),
        },
        Some(s) => HelperStatus {
            state: s.state,
            restarts,
            last_error: s.last_error.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(kv: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: BTreeMap<String, String> = kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn the_monolith_is_the_default_and_split_is_opt_in() {
        assert_eq!(HelperLayout::from_vars(vars(&[])), HelperLayout::Monolith);
        assert_eq!(
            HelperLayout::from_vars(vars(&[(LAYOUT_ENV, "1")])),
            HelperLayout::Monolith
        );
        assert_eq!(
            HelperLayout::from_vars(vars(&[(LAYOUT_ENV, "monolith")])),
            HelperLayout::Monolith
        );
        let l = HelperLayout::from_vars(vars(&[(LAYOUT_ENV, "split"), ("QRTZ_ECHO_BIN", "/x/qrtz-echo")]));
        assert!(l.is_split());
        let HelperLayout::Split { programs } = l else {
            unreachable!()
        };
        assert_eq!(programs.get("echo"), Some(&PathBuf::from("/x/qrtz-echo")));
        assert!(!programs.contains_key("nano"));
    }

    #[test]
    fn each_module_runs_its_own_app_next_to_the_helper() {
        let mut base = ProcessSpawn::helper(PathBuf::from("/A/Contents/MacOS/qrtz-helper"));
        base.env.push(("QRTZ_NATIVE_PILL".into(), "1".into()));
        let none = BTreeMap::new();
        let echo = module_spawn(&base, "echo", &none);
        assert_eq!(echo.program, PathBuf::from("/A/Contents/MacOS/qrtz-echo"));
        assert_eq!(echo.args, vec!["--stdio".to_owned()]);
        assert!(echo.env.contains(&("QRTZ_NATIVE_PILL".into(), "1".into())));
        assert!(echo.env.contains(&(MODULE_ENV.into(), "echo".into())));
        let over = BTreeMap::from([("nano".to_owned(), PathBuf::from("/B/qrtz-nano"))]);
        assert_eq!(
            module_spawn(&base, "nano", &over).program,
            PathBuf::from("/B/qrtz-nano")
        );
        let mock = ProcessSpawn::helper(PathBuf::from("/t/qrtz-mock-helper"));
        assert_eq!(
            module_spawn(&mock, "ctrl", &none).program,
            PathBuf::from("/t/qrtz-mock-helper")
        );
    }

    #[test]
    fn split_status_is_running_only_when_every_helper_is() {
        let st = |state, restarts, e: Option<&str>| HelperStatus {
            state,
            restarts,
            last_error: e.map(str::to_owned),
        };
        let all = [
            st(HelperState::Running, 1, None),
            st(HelperState::Backoff, 2, Some("echo exited")),
            st(HelperState::Running, 0, None),
        ];
        assert_eq!(aggregate(&all), st(HelperState::Backoff, 3, Some("echo exited")));
        let up = [
            st(HelperState::Running, 0, None),
            st(HelperState::Running, 1, None),
        ];
        assert_eq!(aggregate(&up), st(HelperState::Running, 1, None));
    }
}
