//! Real child process spawning for the supervisor (tokio::process; not the shell plugin's sidecar API,
//! because the supervisor needs its own heartbeat, kill and backoff control).
use crate::protocol::MAX_LINE;
use crate::supervisor::{ChildIo, ExitInfo, Spawn};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub struct ProcessSpawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl ProcessSpawn {
    /// The helper is always launched as `qrtz-helper --stdio`.
    pub fn helper(program: PathBuf) -> Self {
        Self {
            program,
            args: vec!["--stdio".into()],
            env: vec![],
        }
    }
}

#[cfg(unix)]
fn signal_name(st: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    st.signal().map(|s| match s {
        9 => "SIGKILL".to_string(),
        15 => "SIGTERM".to_string(),
        6 => "SIGABRT".to_string(),
        11 => "SIGSEGV".to_string(),
        n => format!("SIG{n}"),
    })
}
#[cfg(not(unix))]
fn signal_name(_: &std::process::ExitStatus) -> Option<String> {
    None
}

impl Spawn for ProcessSpawn {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .envs(self.env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // ADR 0046 §1, N55: the speech socket's helper end at fd 3 (`QRTZ_SPEECH_FD=3`); the host keeps the other.
        #[cfg(unix)]
        let speech = {
            use std::os::fd::AsRawFd;
            let (host, helper) = crate::fdpass::socketpair()?;
            let raw = helper.as_raw_fd();
            cmd.env(
                crate::fdpass::SPEECH_FD_ENV,
                crate::fdpass::HELPER_SPEECH_FD.to_string(),
            );
            // SAFETY: only async-signal-safe dup2 runs between fork and exec; dup2 clears CLOEXEC on fd 3.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::dup2(raw, crate::fdpass::HELPER_SPEECH_FD) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            Some((host, helper))
        };
        #[cfg(not(unix))]
        let speech: Option<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> = None;
        let mut child = cmd.spawn()?;
        // The helper end now lives in the child only.
        let speech = speech.map(|(host, _helper)| host);
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stderr = child.stderr.take().expect("piped stderr");
        let (line_tx, line_rx) = mpsc::unbounded_channel();
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<String>();
        let (exit_tx, exit_rx) = oneshot::channel();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            let mut r = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match r.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if buf.len() > MAX_LINE {
                            continue;
                        }
                        let line = String::from_utf8_lossy(&buf);
                        let line = line.trim_end_matches(['\n', '\r']).to_owned();
                        if line_tx.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        tokio::spawn(async move {
            while let Some(line) = in_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!(target: "qrtz_helper_stderr", "{l}");
            }
        });
        tokio::spawn(async move {
            let status = tokio::select! {
                s = child.wait() => s,
                _ = kill_rx => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            let info = match status {
                Ok(st) => ExitInfo {
                    code: st.code(),
                    signal: signal_name(&st),
                },
                Err(_) => ExitInfo {
                    code: None,
                    signal: Some("EWAIT".into()),
                },
            };
            let _ = exit_tx.send(info);
        });
        Ok(ChildIo {
            lines: line_rx,
            stdin: in_tx,
            exit: exit_rx,
            kill: kill_tx,
            speech,
        })
    }
}
