//! Port of packages/core/src/helper/supervisor.ts on tokio: restart with exponential backoff (±20% jitter,
//! capped), a circuit breaker (more than `max_restarts` exits inside `window` → `failed`), and a heartbeat
//! watchdog (no line within `heartbeat_timeout` → kill + restart). The child's stdio protocol is untouched:
//! lines go out and come in verbatim.
use serde::Serialize;
use std::collections::VecDeque;
use std::future::pending;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::{sleep_until, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperState {
    Stopped,
    Starting,
    Running,
    Backoff,
    Failed,
}

/// Same JSON as the TS `HelperStatus` (helper:status channel and event).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HelperStatus {
    pub state: HelperState,
    pub restarts: u32,
    #[serde(rename = "lastError", skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

/// One running child: lines out of stdout, lines into stdin, exit notification, kill switch.
pub struct ChildIo {
    pub lines: mpsc::UnboundedReceiver<String>,
    pub stdin: mpsc::UnboundedSender<String>,
    pub exit: oneshot::Receiver<ExitInfo>,
    pub kill: oneshot::Sender<()>,
}

pub trait Spawn: Send + Sync + 'static {
    fn spawn(&self) -> std::io::Result<ChildIo>;
}

#[derive(Debug, Clone)]
pub struct SupervisorOptions {
    pub name: String,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub max_restarts: usize,
    pub window: Duration,
    /// `Duration::ZERO` disables the watchdog.
    pub heartbeat_timeout: Duration,
}

impl Default for SupervisorOptions {
    fn default() -> Self {
        Self {
            name: "qrtz-helper".into(),
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            max_restarts: 5,
            window: Duration::from_secs(60),
            heartbeat_timeout: Duration::from_secs(15),
        }
    }
}

/// round(min(max, base·2^attempt) · (0.8 + random·0.4)), exactly like `backoffDelay` in TS.
pub fn backoff_delay(attempt: u32, base_ms: u64, max_ms: u64, random: f64) -> u64 {
    let exp = (base_ms as f64 * 2f64.powi(attempt.min(62) as i32)).min(max_ms as f64);
    (exp * (0.8 + random * 0.4)).round() as u64
}

enum Cmd {
    Start,
    Stop,
    Send(String, oneshot::Sender<bool>),
    Shutdown(String, Duration, oneshot::Sender<()>),
}

/// Handle to the supervisor actor. Cheap to clone.
#[derive(Clone)]
pub struct Supervisor {
    cmd: mpsc::UnboundedSender<Cmd>,
    status: watch::Receiver<HelperStatus>,
    lines: broadcast::Sender<String>,
}

pub type Random = Arc<dyn Fn() -> f64 + Send + Sync>;

impl Supervisor {
    pub fn spawn(opts: SupervisorOptions, spawner: Arc<dyn Spawn>) -> Self {
        Self::spawn_with_random(opts, spawner, Arc::new(rand::random::<f64>))
    }

    pub fn spawn_with_random(opts: SupervisorOptions, spawner: Arc<dyn Spawn>, random: Random) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let initial = HelperStatus {
            state: HelperState::Stopped,
            restarts: 0,
            last_error: None,
        };
        let (status_tx, status_rx) = watch::channel(initial);
        let (lines_tx, _) = broadcast::channel(4096);
        let actor = Actor {
            opts,
            spawner,
            random,
            state: HelperState::Stopped,
            restart_times: VecDeque::new(),
            attempt: 0,
            last_error: None,
            wanted: false,
            child: None,
            lines_rx: None,
            exit_rx: None,
            hb_deadline: None,
            backoff_deadline: None,
            status_tx,
            lines_tx: lines_tx.clone(),
        };
        tokio::spawn(actor.run(cmd_rx));
        Self {
            cmd: cmd_tx,
            status: status_rx,
            lines: lines_tx,
        }
    }

    pub fn start(&self) {
        let _ = self.cmd.send(Cmd::Start);
    }
    pub fn stop(&self) {
        let _ = self.cmd.send(Cmd::Stop);
    }
    /// Writes one line (must end with '\n') if the helper is running. False otherwise (TS `send()`).
    pub async fn send(&self, line: String) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.cmd.send(Cmd::Send(line, tx)).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }
    /// Sync, ordered enqueue (the runtime's `HelperPort::send` is sync): true if the helper is running right now.
    /// The line goes through the same actor queue as [`Supervisor::send`], so order with other sends is kept.
    pub fn try_send(&self, line: String) -> bool {
        if self.status.borrow().state != HelperState::Running {
            return false;
        }
        let (tx, _rx) = oneshot::channel();
        self.cmd.send(Cmd::Send(line, tx)).is_ok()
    }
    /// `helper.stop(); helper.start()`: a fresh helper process (the old one may still hold a stuck writer).
    pub fn restart(&self) {
        let _ = self.cmd.send(Cmd::Stop);
        let _ = self.cmd.send(Cmd::Start);
    }
    /// Graceful quit: write `goodbye` (e.g. the `shutdown` message), wait up to `grace` for exit, then kill.
    pub async fn shutdown(&self, goodbye: String, grace: Duration) {
        let (tx, rx) = oneshot::channel();
        if self.cmd.send(Cmd::Shutdown(goodbye, grace, tx)).is_ok() {
            let _ = rx.await;
        }
    }
    pub fn status(&self) -> HelperStatus {
        self.status.borrow().clone()
    }
    pub fn watch_status(&self) -> watch::Receiver<HelperStatus> {
        self.status.clone()
    }
    pub fn subscribe_lines(&self) -> broadcast::Receiver<String> {
        self.lines.subscribe()
    }
}

struct Running {
    stdin: mpsc::UnboundedSender<String>,
    kill: Option<oneshot::Sender<()>>,
}

impl Running {
    fn kill(&mut self) {
        if let Some(k) = self.kill.take() {
            let _ = k.send(());
        }
    }
}

enum Wake {
    Cmd(Option<Cmd>),
    Line(Option<String>),
    Exit(ExitInfo),
    Heartbeat,
    Backoff,
}

struct Actor {
    opts: SupervisorOptions,
    spawner: Arc<dyn Spawn>,
    random: Random,
    state: HelperState,
    restart_times: VecDeque<Instant>,
    attempt: u32,
    last_error: Option<String>,
    wanted: bool,
    child: Option<Running>,
    /// Owned separately from `child` so select! can poll both; cleared whenever the child is dropped,
    /// which is the Rust form of TS's `if (this.child !== child) return` stale-event guard.
    lines_rx: Option<mpsc::UnboundedReceiver<String>>,
    exit_rx: Option<oneshot::Receiver<ExitInfo>>,
    hb_deadline: Option<Instant>,
    backoff_deadline: Option<Instant>,
    status_tx: watch::Sender<HelperStatus>,
    lines_tx: broadcast::Sender<String>,
}

async fn next_line(rx: &mut Option<mpsc::UnboundedReceiver<String>>) -> Option<String> {
    match rx.as_mut() {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}
async fn next_exit(rx: &mut Option<oneshot::Receiver<ExitInfo>>) -> ExitInfo {
    match rx.as_mut() {
        Some(rx) => rx.await.unwrap_or(ExitInfo {
            code: None,
            signal: Some("LOST".into()),
        }),
        None => pending().await,
    }
}
async fn at(deadline: Option<Instant>) {
    match deadline {
        Some(d) => sleep_until(d).await,
        None => pending().await,
    }
}

impl Actor {
    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            let wake = tokio::select! {
                biased;
                c = cmds.recv() => Wake::Cmd(c),
                l = next_line(&mut self.lines_rx) => Wake::Line(l),
                e = next_exit(&mut self.exit_rx) => Wake::Exit(e),
                _ = at(self.hb_deadline) => Wake::Heartbeat,
                _ = at(self.backoff_deadline) => Wake::Backoff,
            };
            match wake {
                Wake::Cmd(None) => {
                    // Every handle dropped: take the child down with us.
                    self.drop_child(true);
                    return;
                }
                Wake::Cmd(Some(c)) => self.command(c).await,
                Wake::Line(Some(line)) => self.on_line(line),
                Wake::Line(None) => self.lines_rx = None, // stdout closed; the exit branch reports the rest
                Wake::Exit(e) => {
                    self.drop_child(false);
                    self.handle_exit(e.code, e.signal, None);
                }
                Wake::Heartbeat => {
                    self.hb_deadline = None;
                    if self.child.is_some() {
                        tracing::warn!(name = %self.opts.name, "heartbeat timeout; killing");
                        self.drop_child(true);
                        self.handle_exit(None, Some("HEARTBEAT".into()), Some("heartbeat timeout".into()));
                    }
                }
                Wake::Backoff => {
                    self.backoff_deadline = None;
                    if self.wanted {
                        self.launch();
                    }
                }
            }
        }
    }

    async fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Start => {
                self.wanted = true;
                if self.child.is_some() || self.state == HelperState::Backoff {
                    return;
                }
                self.restart_times.clear();
                self.attempt = 0;
                self.launch();
            }
            Cmd::Stop => self.stop(),
            Cmd::Send(line, reply) => {
                let ok = match (&self.child, self.state) {
                    (Some(c), HelperState::Running) => c.stdin.send(line).is_ok(),
                    _ => false,
                };
                let _ = reply.send(ok);
            }
            Cmd::Shutdown(goodbye, grace, reply) => {
                self.wanted = false;
                self.backoff_deadline = None;
                self.hb_deadline = None;
                if let Some(mut c) = self.child.take() {
                    self.lines_rx = None;
                    let _ = c.stdin.send(goodbye);
                    if let Some(exit) = self.exit_rx.take() {
                        if tokio::time::timeout(grace, exit).await.is_err() {
                            c.kill();
                        }
                    }
                }
                self.set_state(HelperState::Stopped);
                let _ = reply.send(());
            }
        }
    }

    fn stop(&mut self) {
        self.wanted = false;
        self.backoff_deadline = None;
        self.hb_deadline = None;
        self.drop_child(true);
        self.set_state(HelperState::Stopped);
    }

    fn drop_child(&mut self, kill: bool) {
        self.lines_rx = None;
        self.exit_rx = None;
        if let Some(mut c) = self.child.take() {
            if kill {
                c.kill();
            }
        }
    }

    fn launch(&mut self) {
        self.set_state(HelperState::Starting);
        match self.spawner.spawn() {
            Ok(io) => {
                self.child = Some(Running {
                    stdin: io.stdin,
                    kill: Some(io.kill),
                });
                self.lines_rx = Some(io.lines);
                self.exit_rx = Some(io.exit);
                self.arm_heartbeat();
            }
            Err(e) => self.handle_exit(None, None, Some(e.to_string())),
        }
    }

    fn on_line(&mut self, line: String) {
        if self.state == HelperState::Starting {
            self.attempt = 0;
            self.set_state(HelperState::Running);
        }
        self.arm_heartbeat();
        let _ = self.lines_tx.send(line);
    }

    fn arm_heartbeat(&mut self) {
        self.hb_deadline =
            (!self.opts.heartbeat_timeout.is_zero()).then(|| Instant::now() + self.opts.heartbeat_timeout);
    }

    fn handle_exit(&mut self, code: Option<i32>, signal: Option<String>, error: Option<String>) {
        self.hb_deadline = None;
        self.backoff_deadline = None;
        if !self.wanted {
            return self.set_state(HelperState::Stopped);
        }
        let fmt = |v: Option<String>| v.unwrap_or_else(|| "null".into());
        self.last_error = Some(error.clone().unwrap_or_else(|| {
            format!(
                "exited code={} signal={}",
                fmt(code.map(|c| c.to_string())),
                fmt(signal.clone())
            )
        }));
        tracing::warn!(name = %self.opts.name, ?code, ?signal, ?error, "helper exited");
        let now = Instant::now();
        let window = self.opts.window;
        self.restart_times.retain(|t| now.duration_since(*t) < window);
        if self.restart_times.len() >= self.opts.max_restarts {
            tracing::error!(name = %self.opts.name, restarts = self.restart_times.len(), "crash loop; giving up");
            return self.set_state(HelperState::Failed);
        }
        self.restart_times.push_back(now);
        let delay = backoff_delay(
            self.attempt,
            self.opts.base_delay.as_millis() as u64,
            self.opts.max_delay.as_millis() as u64,
            (self.random)(),
        );
        self.attempt += 1;
        self.set_state(HelperState::Backoff);
        self.backoff_deadline = Some(now + Duration::from_millis(delay));
    }

    fn set_state(&mut self, s: HelperState) {
        self.state = s;
        let status = HelperStatus {
            state: s,
            restarts: self.restart_times.len() as u32,
            last_error: self.last_error.clone(),
        };
        self.status_tx.send_replace(status);
    }
}
