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
