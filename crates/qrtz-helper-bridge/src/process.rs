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

/// The marker apps/desktop/src-tauri/build.rs puts in the shell-script stub it writes for a missing externalBin.
pub const DEV_STUB_MARK: &[u8] = b"qrtz: dev stub";

/// True if `program` is that dev stub (it only prints how to build the real binary and exits 1), so the supervisor
/// reports why instead of crash-looping on an opaque `exited code=1`.
pub fn is_dev_stub(program: &std::path::Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 128];
    let Ok(mut f) = std::fs::File::open(program) else {
        return false;
    };
    let n = f.read(&mut head).unwrap_or(0);
    head[..n].starts_with(b"#!") && head[..n].windows(DEV_STUB_MARK.len()).any(|w| w == DEV_STUB_MARK)
}

#[cfg(unix)]
fn signal_name(st: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    st.signal().map(signal_number_name)
}

/// `SIGKILL`, `SIGTERM`, ... for a signal number (the names the supervisor logs and the UI shows).
pub(crate) fn signal_number_name(n: i32) -> String {
    match n {
        9 => "SIGKILL".to_string(),
        15 => "SIGTERM".to_string(),
        6 => "SIGABRT".to_string(),
        11 => "SIGSEGV".to_string(),
        n => format!("SIG{n}"),
    }
}
#[cfg(not(unix))]
fn signal_name(_: &std::process::ExitStatus) -> Option<String> {
    None
}

/// A helper stderr line is cut to this many bytes before it reaches the log (ADR 0083).
pub const STDERR_LINE_MAX: usize = 2000;

/// The helper's stderr (diagnostics, `logErr`) into the app log under the `helper` target, one event per line.
/// Lines are read with a bounded buffer: a line longer than [`STDERR_LINE_MAX`] is cut, never held whole. The log
/// sink redacts the home directory and escapes control characters.
pub async fn log_stderr<R: tokio::io::AsyncRead + Unpin>(stderr: R) {
    let mut r = BufReader::new(stderr);
    let mut line = Vec::with_capacity(256);
    let mut cut = 0usize;
    loop {
        let (done, used) = match r.fill_buf().await {
            Ok([]) | Err(_) => break,
            Ok(buf) => match buf.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    take(&mut line, &mut cut, &buf[..i]);
                    (true, i + 1)
                }
                None => {
                    take(&mut line, &mut cut, buf);
                    (false, buf.len())
                }
            },
        };
        r.consume(used);
        if done {
            emit_stderr_line(&line, cut);
            line.clear();
            cut = 0;
        }
    }
    if !line.is_empty() {
        emit_stderr_line(&line, cut);
    }
}

fn take(line: &mut Vec<u8>, cut: &mut usize, b: &[u8]) {
    let room = STDERR_LINE_MAX.saturating_sub(line.len());
    line.extend_from_slice(&b[..b.len().min(room)]);
    *cut += b.len().saturating_sub(room);
}

fn emit_stderr_line(line: &[u8], cut: usize) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim_end_matches('\r');
    if text.is_empty() && cut == 0 {
        return;
    }
    if cut > 0 {
        tracing::info!(target: "helper", stream = "stderr", cut, "{text}");
    } else {
        tracing::info!(target: "helper", stream = "stderr", "{text}");
    }
}

impl Spawn for ProcessSpawn {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        if is_dev_stub(&self.program) {
            return Err(std::io::Error::other(format!(
                "{} is the dev stub, not the real binary: run `bun run build:helper`",
                self.program.display()
            )));
        }
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
        // N98: no non-atomic close-on-exec create may be in flight while the helper forks.
        let mut child = qrtz_cloexec::spawn(|| cmd.spawn())?;
        // The helper end now lives in the child only.
        let speech = speech.map(|(host, _helper)| host);
        let unpiped = |what: &str| std::io::Error::other(format!("helper {what} is not piped"));
        let stdout = child.stdout.take().ok_or_else(|| unpiped("stdout"))?;
        let stdin = child.stdin.take().ok_or_else(|| unpiped("stdin"))?;
        let stderr = child.stderr.take().ok_or_else(|| unpiped("stderr"))?;
        Ok(child_io(stdout, stdin, stderr, speech, |kill_rx| async move {
            let status = tokio::select! {
                s = child.wait() => s,
                _ = kill_rx => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            match status {
                Ok(st) => ExitInfo {
                    code: st.code(),
                    signal: signal_name(&st),
                },
                Err(_) => ExitInfo {
                    code: None,
                    signal: Some("EWAIT".into()),
                },
            }
        }))
    }
}

/// A started child's pipes as the supervisor's [`ChildIo`]: stdout lines in (over-long lines dropped), stdin lines
/// out, stderr into the log, and `exit` (given the kill switch: resolved when the supervisor kills the child or
/// drops it) as the exit report. Shared by [`ProcessSpawn::spawn`] and the verified macOS launch
/// ([`crate::suspended`]).
pub(crate) fn child_io<R, W, E, F>(
    stdout: R,
    mut stdin: W,
    stderr: E,
    speech: Option<std::os::fd::OwnedFd>,
    exit: impl FnOnce(oneshot::Receiver<()>) -> F,
) -> ChildIo
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    E: tokio::io::AsyncRead + Unpin + Send + 'static,
    F: std::future::Future<Output = ExitInfo> + Send + 'static,
{
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
    tokio::spawn(log_stderr(stderr));
    let wait = exit(kill_rx);
    tokio::spawn(async move {
        let _ = exit_tx.send(wait.await);
    });
    ChildIo {
        lines: line_rx,
        stdin: in_tx,
        exit: exit_rx,
        kill: kill_tx,
        speech,
    }
}
