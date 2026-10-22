//! The on-device engine (ADR 0046 §3, §6): spawns the pinned module version from its verified fds into the
//! confined fd layout, speaks protocol v1 with it, and turns its untrusted output into validated `SpeechEvent`s.
//!
//! fds in the module: 0 /dev/null, 1 /dev/null, 2 log (append), 3 control (host → module), 4 results (module →
//! host), 5 mic PCM, 6 system PCM (read ends; the write ends go to the helper with `speech.attach`, N55, or are
//! written here for in-process audio and the health check), 7.. model files.
//!
//! N77: audio write ends are `O_NONBLOCK`: a module that stops reading costs dropped frames (counted), never a
//! blocked capture path. The event channel is bounded; a full channel fails the session instead of blocking the
//! reader. Lines are bounded before allocation.
#[cfg(target_os = "linux")]
use crate::confine::Confinement;
use crate::engine::{
    Channel, EngineInfo, EngineKind, FailReason, SessionSpec, SpeechEngine, SpeechEvent, SpeechSession,
};
use crate::files::ModuleFiles;
use crate::protocol::{read_bounded_line, start_message};
use crate::store::SessionPin;
use crate::validate::{check_hello, OutputValidator, Verdict};
use crate::{SpeechError, SpeechStore};
use async_trait::async_trait;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::unix::pipe;
use tokio::sync::{mpsc, Mutex};

/// Bounded event channel (N77).
pub const EVENT_CHANNEL: usize = 1024;
const MAX_LINE: usize = 64 * 1024;

pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct SpawnOptions {
    /// The module's stderr (fd 2), appended, 0600.
    pub log_path: PathBuf,
    pub hello_timeout: Duration,
    /// Dev/test only: run without Landlock/seccomp when the kernel lacks them (fd hygiene still applies).
    /// Release builds refuse instead (ADR 0046 §6).
    pub allow_unconfined: bool,
    /// The module's `log` lines (already schema-checked; never shown to users).
    pub on_log: Option<LogSink>,
    /// Tests only: skip Landlock/seccomp even when available (shows the probes detect access).
    #[doc(hidden)]
    pub force_unconfined: bool,
}

impl SpawnOptions {
    pub fn new(log_path: PathBuf) -> Self {
        SpawnOptions {
            log_path,
            hello_timeout: Duration::from_secs(10),
            allow_unconfined: false,
            on_log: None,
            force_unconfined: false,
        }
    }
}

/// A running module process with its host-side fd ends.
pub struct ModuleProcess {
    pub child: tokio::process::Child,
    pub control: pipe::Sender,
    pub results: BufReader<pipe::Receiver>,
    /// Write ends of fds 5/6 (`O_NONBLOCK`). `None` for a channel the session lacks (the module sees EOF).
    pub mic: Option<OwnedFd>,
    pub system: Option<OwnedFd>,
    pub confined: bool,
}

/// A close-on-exec pipe (N98: atomic on Linux; on macOS created under `qrtz_cloexec`'s spawn lock, so no
/// concurrently spawned host child can inherit it).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the macOS spawner once it lands
fn pipe2() -> std::io::Result<(OwnedFd, OwnedFd)> {
    qrtz_cloexec::pipe()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the macOS spawner once it lands
fn set_nonblocking(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on an fd we own.
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        if fl < 0 || libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the macOS spawner once it lands
fn open_log(p: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(p)
}

/// Spawn the module from verified files (N73: the entry is exec'd from its hashed fd).
pub fn spawn_module(
    files: &ModuleFiles,
    channels: &[Channel],
    opts: &SpawnOptions,
) -> Result<ModuleProcess, SpeechError> {
    #[cfg(target_os = "linux")]
    {
        spawn_linux(files, channels, opts)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (files, channels, opts);
        Err(SpeechError::Unconfined(
            "the macOS module spawner (posix_spawn + sandbox + code requirement) is not built yet".into(),
        ))
    }
}

#[cfg(target_os = "linux")]
fn spawn_linux(
    files: &ModuleFiles,
    channels: &[Channel],
    opts: &SpawnOptions,
) -> Result<ModuleProcess, SpeechError> {
    use crate::confine::{landlock_ruleset, ChildPlan, SeccompFilter};
    use std::os::unix::fs::FileExt;
    let conf = Confinement::probe();
    if !conf.available() && !opts.allow_unconfined {
        return Err(SpeechError::Unconfined(format!(
            "Landlock ABI {} / seccomp {}: speech modules need both",
            conf.landlock_abi, conf.seccomp
        )));
    }
    let devnull_in = std::fs::File::open("/dev/null")?;
    let devnull_out = std::fs::OpenOptions::new().write(true).open("/dev/null")?;
    let log = open_log(&opts.log_path)?;
    let (ctl_r, ctl_w) = pipe2()?;
    let (res_r, res_w) = pipe2()?;
    let (mic_r, mic_w) = pipe2()?;
    let (sys_r, sys_w) = pipe2()?;
    let mut targets: Vec<RawFd> = vec![
        devnull_in.as_raw_fd(),
        devnull_out.as_raw_fd(),
        log.as_raw_fd(),
        ctl_r.as_raw_fd(),
        res_w.as_raw_fd(),
        mic_r.as_raw_fd(),
        sys_r.as_raw_fd(),
    ];
    targets.extend(files.models.iter().map(|f| f.file.as_raw_fd()));
    let entry = files.entry.file.as_raw_fd();
    let mut head = vec![0u8; 64 * 1024];
    let n = files.entry.file.read_at(&mut head, 0)?;
    head.truncate(n);
    let confine = conf.available() && !opts.force_unconfined;
    let (ruleset, seccomp) = if confine {
        (
            Some(landlock_ruleset(conf.landlock_abi, entry, &head)?),
            Some(SeccompFilter::build()),
        )
    } else {
        tracing::warn!("speech module runs unconfined (dev/test only)");
        (None, None)
    };
    let argv = vec![c"qrtz-speech".to_owned()];
    let mut plan = ChildPlan::new(targets, entry, ruleset, seccomp, argv);
    let mut cmd = tokio::process::Command::new("/proc/self/exe");
    cmd.env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    // SAFETY: exec_in_child only makes async-signal-safe calls and allocates nothing.
    unsafe {
        cmd.pre_exec(move || plan.exec_in_child());
    }
    let child = qrtz_cloexec::spawn(|| cmd.spawn())?;
    // N55 (host side): our copies of the child's ends close now.
    drop((ctl_r, res_w, mic_r, sys_r, devnull_in, devnull_out, log));
    set_nonblocking(ctl_w.as_raw_fd())?;
    set_nonblocking(res_r.as_raw_fd())?;
    set_nonblocking(mic_w.as_raw_fd())?;
    set_nonblocking(sys_w.as_raw_fd())?;
    let control = pipe::Sender::from_owned_fd(ctl_w)?;
    let results = BufReader::new(pipe::Receiver::from_owned_fd(res_r)?);
    Ok(ModuleProcess {
        child,
        control,
        results,
        mic: channels.contains(&Channel::Mic).then_some(mic_w),
        system: channels.contains(&Channel::System).then_some(sys_w),
        confined: confine,
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Non-blocking write of one audio frame: `false` = dropped (pipe full or closed), N77.
pub fn write_audio_nonblocking(fd: &OwnedFd, pcm: &[u8]) -> bool {
    // SAFETY: pcm is valid for its length.
    let n = unsafe { libc::write(fd.as_raw_fd(), pcm.as_ptr().cast(), pcm.len()) };
    n == pcm.len() as isize
}

/// Shared between the session and its reader task.
struct Shared {
    control: Mutex<pipe::Sender>,
    session_id: String,
    started: Instant,
    pushed_ms: AtomicU64,
    dropped_ms: AtomicU64,
    ending_revoked: AtomicBool,
}

impl Shared {
    async fn send(&self, v: serde_json::Value) -> Result<(), SpeechError> {
        let mut line = v.to_string();
        line.push('\n');
        let mut c = self.control.lock().await;
        tokio::time::timeout(Duration::from_secs(2), c.write_all(line.as_bytes()))
            .await
            .map_err(|_| SpeechError::Io(std::io::Error::other("module control pipe full")))??;
        Ok(())
    }
    fn audio_ms(&self) -> u64 {
        (self.started.elapsed().as_millis() as u64).max(self.pushed_ms.load(Ordering::SeqCst))
    }
}

/// Hands a session's audio write ends to whoever captures (the helper, `speech.attach`, N55). Resolves once the
/// receiver acknowledged; the fds are consumed either way (the host keeps no copy).
#[async_trait]
pub trait AudioAttach: Send + Sync {
    async fn attach(&self, spec: &SessionSpec, fds: Vec<(Channel, OwnedFd)>) -> Result<(), SpeechError>;
}

/// The engine over the installed module store.
pub struct OnDeviceSpeech {
    pub store: Arc<SpeechStore>,
    pub opts: SpawnOptions,
    /// `Some` = live sessions get their audio from the helper; `None` = the host pushes audio (tests, health).
    pub attach: Option<Arc<dyn AudioAttach>>,
}

impl OnDeviceSpeech {
    pub fn new(store: Arc<SpeechStore>, opts: SpawnOptions) -> Self {
        OnDeviceSpeech {
            store,
            opts,
            attach: None,
        }
    }
    pub fn with_attach(mut self, attach: Arc<dyn AudioAttach>) -> Self {
        self.attach = Some(attach);
        self
    }
}

pub struct ModuleSession {
    version: String,
    shared: Arc<Shared>,
    rx: mpsc::Receiver<SpeechEvent>,
    mic: Option<OwnedFd>,
    system: Option<OwnedFd>,
    child: Arc<Mutex<tokio::process::Child>>,
    _pin: Option<Arc<SessionPin>>,
    pub confined: bool,
}

impl ModuleSession {
    /// Frames dropped so far because the module did not keep up (N77), in ms of audio.
    pub fn dropped_ms(&self) -> u64 {
        self.shared.dropped_ms.load(Ordering::SeqCst)
    }
    /// Hand the audio write ends to the helper (`speech.attach`, N55): the host keeps no copy.
    pub fn take_audio_fds(&mut self) -> (Option<OwnedFd>, Option<OwnedFd>) {
        (self.mic.take(), self.system.take())
    }
    /// The health check writes the whole clip (waiting for the pipe), unlike live audio.
    pub async fn write_clip(&mut self, ch: Channel, pcm: &[u8]) -> Result<(), SpeechError> {
        let fd = match ch {
            Channel::Mic => self.mic.as_ref(),
            Channel::System => self.system.as_ref(),
        }
        .ok_or_else(|| SpeechError::Invalid("channel not in session".into()))?;
        // Counted up front for the same reason as `push_audio` (QRT-336); a clip that is not fully read
        // fails the check.
        self.shared
            .pushed_ms
            .fetch_max((pcm.len() / 32) as u64, Ordering::SeqCst);
        let mut off = 0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while off < pcm.len() {
            // SAFETY: valid slice.
            let n = unsafe { libc::write(fd.as_raw_fd(), pcm[off..].as_ptr().cast(), pcm.len() - off) };
            if n > 0 {
                off += n as usize;
            } else if Instant::now() > deadline {
                return Err(SpeechError::Health("module did not read the clip".into()));
            } else {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl SpeechSession for ModuleSession {
    fn version(&self) -> &str {
        &self.version
    }
    fn events(&mut self) -> &mut mpsc::Receiver<SpeechEvent> {
        &mut self.rx
    }
    async fn push_audio(&mut self, ch: Channel, pcm16: &[u8]) -> Result<(), SpeechError> {
        let fd = match ch {
            Channel::Mic => self.mic.as_ref(),
            Channel::System => self.system.as_ref(),
        };
        let Some(fd) = fd else { return Ok(()) };
        let ms = (pcm16.len() / 32) as u64;
        // Count the audio before the module can see it (QRT-336): a module that answers before this task
        // resumes must have its words checked against audio that includes them, or they are dropped as
        // "ahead of the audio" protocol violations.
        self.shared.pushed_ms.fetch_add(ms, Ordering::SeqCst);
        if !write_audio_nonblocking(fd, pcm16) {
            self.shared.pushed_ms.fetch_sub(ms, Ordering::SeqCst);
            self.shared.dropped_ms.fetch_add(ms, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn finish(&mut self) -> Result<(), SpeechError> {
        // EOF on the audio fds, then `finish`.
        self.mic = None;
        self.system = None;
        let id = self.shared.session_id.clone();
        self.shared
            .send(serde_json::json!({"type": "finish", "sessionId": id}))
            .await
    }
    async fn cancel(&mut self) {
        let id = self.shared.session_id.clone();
        let _ = self
            .shared
            .send(serde_json::json!({"type": "cancel", "sessionId": id}))
            .await;
        let child = self.child.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = child.lock().await.start_kill();
        });
    }
}

/// Start a session on already-opened files (the store's pin, or a health-check candidate).
pub async fn start_on_files(
    version: &str,
    files: ModuleFiles,
    spec: SessionSpec,
    opts: &SpawnOptions,
    pin: Option<Arc<SessionPin>>,
    store: Option<Arc<SpeechStore>>,
) -> Result<ModuleSession, SpeechError> {
    let mut proc = spawn_module(&files, &spec.channels, opts)?;
    let fd_list: Vec<(String, i32, u64)> = files
        .models
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.clone(), 7 + i as i32, f.size))
        .collect();
    drop(files);
    let start = start_message(&spec, &fd_list).map_err(SpeechError::Invalid)?;
    // Handshake (N57: the module's word on confinement is never asked for).
    let hello =
        tokio::time::timeout(opts.hello_timeout, read_bounded_line(&mut proc.results, MAX_LINE)).await;
    let line = match hello {
        Ok(Ok(Some(Ok(l)))) => l,
        Ok(Ok(_)) | Ok(Err(_)) => {
            let _ = proc.child.start_kill();
            return Err(SpeechError::Health("module exited before hello".into()));
        }
        Err(_) => {
            let _ = proc.child.start_kill();
            return Err(SpeechError::Health("no hello in time".into()));
        }
    };
    if let Err(e) = check_hello(&line, version) {
        let _ = proc.child.start_kill();
        return Err(SpeechError::Health(format!("hello: {e}")));
    }
    let shared = Arc::new(Shared {
        control: Mutex::new(proc.control),
        session_id: spec.session_id.clone(),
        started: Instant::now(),
        pushed_ms: AtomicU64::new(0),
        dropped_ms: AtomicU64::new(0),
        ending_revoked: AtomicBool::new(false),
    });
    shared.send(start).await?;
    let (tx, rx) = mpsc::channel(EVENT_CHANNEL);
    let child = Arc::new(Mutex::new(proc.child));
    let mut validator = OutputValidator::for_session(&spec.session_id, &spec.channels);
    let (sh, ch, pin2, on_log) = (shared.clone(), child.clone(), pin.clone(), opts.on_log.clone());
    let mut results = proc.results;
    tokio::spawn(async move {
        let fail = |reason| SpeechEvent::Failed { reason };
        let mut finished = false;
        let outcome: Option<FailReason> = loop {
            let line = match read_bounded_line(&mut results, MAX_LINE).await {
                Ok(Some(Ok(l))) => l,
                Ok(Some(Err(_))) => "x".repeat(MAX_LINE + 1), // let the validator count it
                Ok(None) | Err(_) => break if finished { None } else { Some(FailReason::Crashed) },
            };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if v["type"] == "log" {
                    if let (Some(f), Some(m)) = (&on_log, v["msg"].as_str()) {
                        f(m);
                    }
                }
            }
            match validator.check(&line, now_ms(), sh.audio_ms()) {
                Verdict::Event(ev) => {
                    let is_final = matches!(ev, SpeechEvent::Words { is_final: true, .. });
                    if let SpeechEvent::Finished { .. } = ev {
                        finished = true;
                        let _ = sh.send(serde_json::json!({"type": "shutdown"})).await;
                        if sh.ending_revoked.load(Ordering::SeqCst) {
                            break Some(FailReason::Revoked);
                        }
                    }
                    if tx.try_send(ev).is_err() {
                        tracing::warn!("speech event channel full or closed; ending the session");
                        break Some(FailReason::ProtocolViolations);
                    }
                    if finished {
                        break None;
                    }
                    // N66 safe point: after a final utterance, a revoked pin ends the session gracefully.
                    if is_final
                        && pin2.as_ref().is_some_and(|p| p.revoked())
                        && !sh.ending_revoked.swap(true, Ordering::SeqCst)
                    {
                        tracing::warn!("speech version revoked during a session; ending at this safe point");
                        let id = sh.session_id.clone();
                        let _ = sh
                            .send(serde_json::json!({"type": "finish", "sessionId": id}))
                            .await;
                    }
                }
                Verdict::Ignore => {}
                Verdict::Dropped(why) => tracing::debug!(%why, "speech event dropped"),
                Verdict::Fail(why) => {
                    tracing::warn!(%why, "speech module output invalid; ending the session");
                    break Some(FailReason::ProtocolViolations);
                }
            }
        };
        match outcome {
            None => {
                if let Some(s) = &store {
                    s.record_success();
                }
            }
            Some(reason) => {
                if matches!(reason, FailReason::Crashed | FailReason::ProtocolViolations) {
                    if let Some(s) = &store {
                        let _ = s.record_failure(now_ms());
                    }
                }
                let _ = tx.try_send(fail(reason));
            }
        }
        // Give the module a moment to exit on shutdown, then make sure it is gone.
        let mut c = ch.lock().await;
        if tokio::time::timeout(Duration::from_secs(2), c.wait())
            .await
            .is_err()
        {
            let _ = c.start_kill();
            let _ = c.wait().await;
        }
    });
    Ok(ModuleSession {
        version: version.to_string(),
        shared,
        rx,
        mic: proc.mic,
        system: proc.system,
        child,
        _pin: pin,
        confined: proc.confined,
    })
}

impl OnDeviceSpeech {
    /// `start` with the concrete session type (tests, the health check, `speech.attach`).
    pub async fn start_module(&self, spec: SessionSpec) -> Result<ModuleSession, SpeechError> {
        let pin = Arc::new(self.store.pin_session(now_ms())?);
        let files = pin.open_files()?;
        let version = pin.version.clone();
        let r = start_on_files(
            &version,
            files,
            spec,
            &self.opts,
            Some(pin),
            Some(self.store.clone()),
        )
        .await;
        if r.is_err() {
            let _ = self.store.record_failure(now_ms());
        }
        r
    }
}

#[async_trait]
impl SpeechEngine for OnDeviceSpeech {
    fn info(&self) -> EngineInfo {
        EngineInfo {
            version: self.store.state().current.unwrap_or_default(),
            kind: EngineKind::OnDevice,
        }
    }
    async fn start(&self, spec: SessionSpec) -> Result<Box<dyn SpeechSession>, SpeechError> {
        let mut s = self.start_module(spec.clone()).await?;
        if let Some(a) = &self.attach {
            let (mut mic, mut system) = s.take_audio_fds();
            let mut fds = vec![];
            for ch in &spec.channels {
                let fd = match ch {
                    Channel::Mic => mic.take(),
                    Channel::System => system.take(),
                };
                if let Some(fd) = fd {
                    fds.push((*ch, fd));
                }
            }
            if let Err(e) = a.attach(&spec, fds).await {
                s.cancel().await;
                return Err(e);
            }
        }
        Ok(Box::new(s))
    }
}

/// The golden-clip health check (ADR 0046 §5): run the candidate on its clip as a dictation and compare the final
/// words with the manifest's `health.expected` (case- and punctuation-insensitive).
pub struct ModuleHealth {
    pub opts: SpawnOptions,
}

#[cfg(feature = "net")]
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(feature = "net")]
#[async_trait]
impl crate::update::HealthCheck for ModuleHealth {
    async fn check(&self, m: &crate::bundle::Manifest, files: ModuleFiles) -> Result<(), String> {
        use std::os::unix::fs::FileExt;
        let mut clip = vec![0u8; files.clip.size as usize];
        files
            .clip
            .file
            .read_exact_at(&mut clip, 0)
            .map_err(|e| e.to_string())?;
        let spec = SessionSpec {
            session_id: "health".into(),
            kind: crate::engine::SessionKind::Dictation,
            channels: vec![Channel::Mic],
            language_hint: None,
            speaker_mode: crate::engine::SpeakerMode::Fast,
        };
        let mut s = start_on_files(&m.version, files, spec, &self.opts, None, None)
            .await
            .map_err(|e| e.to_string())?;
        s.write_clip(Channel::Mic, &clip)
            .await
            .map_err(|e| e.to_string())?;
        s.finish().await.map_err(|e| e.to_string())?;
        let mut words = vec![];
        let r = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(ev) = s.rx.recv().await {
                match ev {
                    SpeechEvent::Words { is_final: true, .. } => words.push(ev.text().unwrap_or_default()),
                    SpeechEvent::Finished { .. } => return Ok(()),
                    SpeechEvent::Failed { reason } => return Err(format!("{reason:?}")),
                    _ => {}
                }
            }
            Err("module ended without finishing".to_string())
        })
        .await
        .map_err(|_| "health check timed out".to_string())?;
        r?;
        let got = norm(&words.join(" "));
        if got != norm(&m.health.expected) {
            return Err("golden clip words differ".into());
        }
        Ok(())
    }
}
