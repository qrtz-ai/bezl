//! ctrl over the real helper process (slice 5b, ADR 0110): the `qrtz_ctrl::helper::Wire` behind `HelperClient`,
//! carried on the same supervised `qrtz-helper` stdio as echo/nano. ctrl lines are not in the echo/nano schema
//! (`contracts/helper-protocol.json`), so they go out verbatim with `try_send` and come back on the supervisor's
//! line broadcast, matched by `type` prefix `ctrl.` and `id`; `AppService::on_helper_line` skips them.
//!
//! `roundtrip` is sync (the ctrl service runs on `spawn_blocking`); it never blocks the async runtime: the wait
//! is a std channel fed by a task on the runtime, bounded by a timeout, so a helper that never answers (an older
//! build, the Linux mock) costs at most the timeout and the run asks instead of hanging.
use qrtz_helper_bridge::{HelperState, Supervisor};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::broadcast;

/// The Mac's first preferred language (`de-DE`, `en-GB`), what apps localize their labels to; `""` if unread.
/// ctrl's gate reads only English labels, so anything else makes every press confirm (`gate::NON_ENGLISH_RULE`).
#[cfg(target_os = "macos")]
pub fn ui_language() -> String {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> CFArrayRef;
    }
    // SAFETY: CoreFoundation call with no arguments; it returns a +1 array or null.
    let langs = unsafe { CFLocaleCopyPreferredLanguages() };
    if langs.is_null() {
        return String::new();
    }
    // SAFETY: a +1 array of CFStrings from a Copy function.
    let langs: CFArray<CFString> = unsafe { CFArray::wrap_under_create_rule(langs) };
    langs.get(0).map(|l| l.to_string()).unwrap_or_default()
}

/// Off macOS ctrl has no helper and never acts; unread, so the gate would fail closed.
#[cfg(not(target_os = "macos"))]
pub fn ui_language() -> String {
    String::new()
}

pub struct SupervisorWire {
    sup: Supervisor,
    rt: tokio::runtime::Handle,
    timeout: Duration,
}

impl SupervisorWire {
    pub fn new(sup: Supervisor, rt: tokio::runtime::Handle) -> Self {
        SupervisorWire {
            sup,
            rt,
            timeout: Duration::from_secs(10),
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }
}

/// `Some(id)` if `line` is a ctrl message (`type` starts with `ctrl.`).
pub fn ctrl_line_id(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let ty = v.get("type")?.as_str()?;
    ty.starts_with("ctrl.")
        .then(|| v.get("id").and_then(Value::as_str).unwrap_or_default().to_owned())
}

impl qrtz_ctrl::helper::Wire for SupervisorWire {
    fn roundtrip(&self, line: &str) -> Result<String, String> {
        let want = ctrl_line_id(line).ok_or_else(|| "helper.bad_request".to_string())?;
        if self.sup.status().state != HelperState::Running {
            return Err("helper.not_running".into());
        }
        // Subscribe before sending so the reply can't slip past.
        let mut lines = self.sup.subscribe_lines();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let timeout = self.timeout;
        let w = want.clone();
        self.rt.spawn(async move {
            let wait = async {
                loop {
                    match lines.recv().await {
                        Ok(l) => {
                            if l.contains("\"ctrl.") && ctrl_line_id(&l).as_deref() == Some(w.as_str()) {
                                let _ = tx.send(l);
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => return,
                    }
                }
            };
            let _ = tokio::time::timeout(timeout, wait).await;
        });
        let mut out = line.trim_end().to_owned();
        out.push('\n');
        if !self.sup.try_send(out) {
            return Err("helper.not_running".into());
        }
        rx.recv_timeout(timeout + Duration::from_millis(200))
            .map_err(|_| "helper.timeout".to_string())
    }

    fn up(&self) -> bool {
        self.sup.status().state == HelperState::Running
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn the_ui_language_is_a_language_tag() {
        let l = super::ui_language();
        assert!(
            l.len() >= 2
                && l.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "{l:?}"
        );
    }
}
