//! ADR 0231 amendment 1: the app's side of Agent Dash's network helper, `qrtz-dash-net`.
//!
//! qrtz-app stays offline: it links no network crate and never opens a socket. When dash is on (debug builds,
//! [`crate::dash_port::DASH_ENABLED`]) and the user opted in (`dash:connect`), it spawns the helper with an argv
//! and no shell, a cleared environment, and stdio pipes, and passes it `dash:*` calls one JSON line at a time, each
//! with its contract capability (`dash.read` / `dash.manage`). Only `dash:*` parameters go in; only the server's
//! (already scrubbed) dash view and results come out. The helper keeps the dashboard's owner key; the app never
//! sees it.
use qrtz_ipc::{AppError, ErrorCode};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

pub const HELPER_NAME: &str = "qrtz-dash-net";
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REPLY: usize = 1_000_000;

fn unavailable(m: &str) -> AppError {
    AppError::new(ErrorCode::Unavailable, m).retryable(true)
}

/// The helper next to the app's own executable (target/debug in dev; never in a release bundle: verify-bundle.sh).
pub fn default_helper_path() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join(HELPER_NAME);
    p.is_file().then_some(p)
}

/// One line of at most `max` bytes (`read_until` capped at `max + 1`); `None` at EOF or when the line is longer
/// (the caller then drops the helper, so the rest is never read).
pub async fn read_line_capped<R: AsyncBufRead + Unpin>(
    r: &mut R,
    max: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let n = (&mut *r).take(max as u64 + 1).read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > max {
        return Ok(None);
    }
    Ok(Some(buf))
}

pub struct DashNet {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl DashNet {
    pub fn spawn(path: &std::path::Path) -> Result<Self, AppError> {
        let mut cmd = tokio::process::Command::new(path);
        cmd.env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        // The Keychain needs the user's home; nothing else from the app's environment goes across.
        if let Some(h) = std::env::var_os("HOME") {
            cmd.env("HOME", h);
        }
        // One retry on ETXTBSY (a just-written binary still open for writing in a forking process).
        const ETXTBSY: i32 = 26;
        let mut child = match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) => {
                std::thread::sleep(Duration::from_millis(50));
                cmd.spawn()
            }
            r => r,
        }
        .map_err(|_| unavailable("the dash helper can't start"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| unavailable("the dash helper can't start"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| unavailable("the dash helper can't start"))?;
        Ok(DashNet {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next: 1,
        })
    }

    /// One `dash:*` call. `capability` is the channel's contract capability.
    pub async fn call(&mut self, channel: &str, capability: &str, params: &Value) -> Result<Value, AppError> {
        let id = self.next;
        self.next += 1;
        // The line may carry the setup token: zeroed when dropped (Bozniak L).
        let mut line = zeroize::Zeroizing::new(
            json!({"id": id, "channel": channel, "capability": capability, "params": params}).to_string(),
        );
        line.push('\n');
        let gone = || unavailable("the dash helper stopped");
        self.stdin.write_all(line.as_bytes()).await.map_err(|_| gone())?;
        self.stdin.flush().await.map_err(|_| gone())?;
        // Bozniak M2: the reply line is read with a byte limit, never in full.
        let reply = tokio::time::timeout(TIMEOUT, read_line_capped(&mut self.stdout, MAX_REPLY))
            .await
            .map_err(|_| unavailable("the dash helper timed out"))?
            .map_err(|_| gone())?
            .ok_or_else(gone)?;
        let v: Value = serde_json::from_slice(&reply).map_err(|_| gone())?;
        if v["id"] != json!(id) {
            return Err(gone());
        }
        if let Some(r) = v.get("ok") {
            return Ok(r.clone());
        }
        let e = &v["error"];
        let code = e["code"].as_str().unwrap_or("");
        let code = ErrorCode::ALL
            .into_iter()
            .find(|c| c.as_str() == code)
            .unwrap_or(ErrorCode::Unavailable);
        Err(
            AppError::new(code, e["message"].as_str().unwrap_or("the dash server failed"))
                .retryable(e["retryable"].as_bool().unwrap_or(false)),
        )
    }

    pub async fn stop(mut self) {
        drop(self.stdin);
        if tokio::time::timeout(Duration::from_secs(2), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reply_lines_are_read_with_a_byte_limit() {
        let mut big = vec![b'a'; 100];
        big.push(b'\n');
        let mut r = BufReader::new(&big[..]);
        assert!(read_line_capped(&mut r, 99).await.unwrap().is_none());
        let mut r = BufReader::new(&big[..]);
        assert_eq!(read_line_capped(&mut r, 100).await.unwrap().unwrap().len(), 100);
        let mut r = BufReader::new(&b""[..]);
        assert!(read_line_capped(&mut r, 10).await.unwrap().is_none());
    }
}
