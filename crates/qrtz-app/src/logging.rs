//! Local-only logging (ADR 0083): `tracing` to stderr and to a size-capped, rotating, owner-only file
//! (`~/Library/Logs/qrtz/qrtz.log` on macOS). Nothing here opens a socket; the logs leave the Mac only when the
//! user exports them (`logs:export`) and sends the file themselves.
//!
//! Privacy (confinement requirement): call sites log ids, counts, lengths, codes and states, never transcript,
//! dictated or summary text, notes, meeting titles, clipboard contents or secrets. The sink adds a second line of
//! defence that does not depend on call sites: the user's home directory becomes `~`, control and bidi characters
//! are escaped (one event = one line; no log injection, no terminal escapes), and a line is cut at
//! [`MAX_LINE_BYTES`]. Third-party crates log at `warn` at most, whatever `QRTZ_LOG` says, in a release build.
//!
//! Writing never blocks the async runtime: a formatted line goes into a bounded queue (`QUEUE_LINES`) that a
//! dedicated thread drains into the file; a full queue drops the line and the thread notes how many it dropped.
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// The live file; rotated copies are `qrtz.1.log` (newest) … `qrtz.{ROTATED_FILES}.log` (oldest).
pub const LOG_FILE: &str = "qrtz.log";
/// One file's cap. With [`ROTATED_FILES`] the folder never holds more than 5 × 8 MiB = 40 MiB.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub const ROTATED_FILES: usize = 4;
/// One line's cap (after redaction); the rest is cut and the cut is marked.
pub const MAX_LINE_BYTES: usize = 8 * 1024;
/// Lines waiting for the writer thread; past this, new lines are dropped (never the caller blocked).
pub const QUEUE_LINES: usize = 4096;
/// `tracing` targets of the Swift helper (stderr and its `log` protocol lines) and of the renderer (`log:write`).
pub const HELPER_TARGET: &str = "helper";
pub const RENDERER_TARGET: &str = "renderer";
/// A helper `log` / stderr line, and a renderer message, are cut to this many characters before they are logged.
pub const HELPER_MESSAGE_MAX: usize = 2000;
pub const RENDERER_MESSAGE_MAX: usize = 1000;
/// `log:write`: at most this many lines per window; the rest are dropped and counted.
pub const RENDERER_LINES_PER_WINDOW: u32 = 60;
pub const RENDERER_WINDOW: Duration = Duration::from_secs(10);

const LEVELS: &[&str] = &["off", "error", "warn", "info", "debug", "trace"];

/// Where the log folder is. `QRTZ_USER_DATA` (tests, side-by-side runs) keeps logs inside that profile; otherwise
/// the platform's log location: `~/Library/Logs/qrtz` on macOS, `%LOCALAPPDATA%\qrtz\logs` on Windows,
/// `$XDG_STATE_HOME/qrtz/logs` (default `~/.local/state/qrtz/logs`) elsewhere.
pub fn log_dir(os: &str, home: Option<&Path>, env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(d) = var("QRTZ_USER_DATA") {
        return Some(PathBuf::from(d).join("logs"));
    }
    match os {
        "macos" => home.map(|h| h.join("Library/Logs/qrtz")),
        "windows" => var("LOCALAPPDATA").map(|d| PathBuf::from(d).join("qrtz").join("logs")),
        _ => var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".local/state")))
            .map(|d| d.join("qrtz").join("logs")),
    }
}

/// The current user's home directory, from the environment.
pub fn home_dir() -> Option<PathBuf> {
    let k = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `QRTZ_LOG` → filter directives. A debug build takes any `EnvFilter` string. A release build takes a bare level
/// only (`error` … `trace`, or `off`); anything else is ignored. Either way, by default and in a release build,
/// only qrtz's own targets (`qrtz*`, `helper`, `renderer`) log below `warn`.
pub fn filter_directives(env: Option<&str>, release: bool) -> String {
    let env = env.map(str::trim).filter(|v| !v.is_empty());
    if !release {
        if let Some(e) = env {
            return e.to_owned();
        }
    }
    let level = env
        .map(str::to_ascii_lowercase)
        .filter(|l| LEVELS.contains(&l.as_str()))
        .unwrap_or_else(|| "info".into());
    if level == "off" {
        return level;
    }
    let others = if level == "error" { "error" } else { "warn" };
    format!("{others},qrtz={level},{HELPER_TARGET}={level},{RENDERER_TARGET}={level}")
}

/// Line-level redaction applied to every sink.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    home: Option<String>,
}

fn is_bidi(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

impl Redactor {
    /// `home` is replaced by `~` (a root or empty home is ignored).
    pub fn new(home: Option<&Path>) -> Self {
        let home = home
            .map(|h| h.to_string_lossy().trim_end_matches(['/', '\\']).to_owned())
            .filter(|h| h.len() > 1);
        Self { home }
    }

    /// One formatted event → one safe line (no trailing newline): home → `~`, control/bidi characters escaped
    /// (tab kept), cut at [`MAX_LINE_BYTES`].
    pub fn line(&self, s: &str) -> String {
        let s = s.trim_end_matches(['\n', '\r']);
        let homed = match &self.home {
            Some(h) => replace_home(s, h),
            None => s.to_owned(),
        };
        let mut out = String::with_capacity(homed.len().min(MAX_LINE_BYTES) + 16);
        let mut cut = 0usize;
        for c in homed.chars() {
            if cut > 0 || out.len() >= MAX_LINE_BYTES {
                cut += c.len_utf8();
                continue;
            }
            match c {
                '\t' => out.push(c),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                c if c.is_control() || is_bidi(c) => {
                    out.push_str(&format!("\\u{{{:x}}}", c as u32));
                }
                c => out.push(c),
            }
        }
        if cut > 0 {
            out.push_str(&format!(" …[cut {cut} bytes]"));
        }
        out
    }
}

/// `home` as a path prefix (followed by a separator, a quote, whitespace or the end) → `~`.
fn replace_home(s: &str, home: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(home) {
        let after = &rest[i + home.len()..];
        let boundary = after.chars().next().is_none_or(|c| {
            matches!(c, '/' | '\\' | '"' | '\'' | ')' | ']' | ',' | ':' | ';') || c.is_whitespace()
        });
        out.push_str(&rest[..i]);
        out.push_str(if boundary { "~" } else { home });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Cut `s` to at most `max` characters on a char boundary.
pub fn cut_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(unix)]
pub(crate) fn owner_only_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    // A file an older build (or a wider umask) created is tightened too.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(f)
}
#[cfg(not(unix))]
pub(crate) fn owner_only_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn rotated(dir: &Path, i: usize) -> PathBuf {
    dir.join(format!("qrtz.{i}.log"))
}

/// The log files in `dir`, oldest first (`qrtz.4.log` … `qrtz.1.log`, `qrtz.log`); only those that exist.
pub fn log_files(dir: &Path) -> Vec<PathBuf> {
    (1..=ROTATED_FILES)
        .rev()
        .map(|i| rotated(dir, i))
        .chain(std::iter::once(dir.join(LOG_FILE)))
        .filter(|p| p.is_file())
        .collect()
}

/// `qrtz.log` with size-based rotation: past `max_bytes` it becomes `qrtz.1.log`, older copies shift up, and
/// `qrtz.{keep}.log` is deleted. The folder is 0700 and every file 0600.
pub struct RotatingFile {
    dir: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: File,
    size: u64,
}

impl RotatingFile {
    pub fn open(dir: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        crate::dirs::private_dir(dir)?;
        let file = owner_only_file(&dir.join(LOG_FILE))?;
        let size = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            max_bytes,
            keep,
            file,
            size,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        let _ = std::fs::remove_file(rotated(&self.dir, self.keep));
        for i in (1..self.keep).rev() {
            let from = rotated(&self.dir, i);
            if from.exists() {
                std::fs::rename(&from, rotated(&self.dir, i + 1))?;
            }
        }
        if self.keep > 0 {
            std::fs::rename(self.dir.join(LOG_FILE), rotated(&self.dir, 1))?;
        } else {
            std::fs::remove_file(self.dir.join(LOG_FILE))?;
        }
        self.file = owner_only_file(&self.dir.join(LOG_FILE))?;
        self.size = 0;
        Ok(())
    }

    /// Append one line (`line` carries its newline).
    pub fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        if self.size > 0 && self.size + line.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(line)?;
        self.size += line.len() as u64;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

enum Msg {
    Line(Vec<u8>),
    Flush(SyncSender<()>),
}

/// The file sink's front end: a `MakeWriter` whose writers hand finished lines to the writer thread without
/// blocking (`try_send`; a full queue drops the line and counts it).
#[derive(Clone)]
pub struct NonBlocking {
    tx: SyncSender<Msg>,
    dropped: std::sync::Arc<AtomicU64>,
    redact: Redactor,
}

impl NonBlocking {
    /// Start the writer thread over `file`.
    pub fn spawn(file: RotatingFile, queue: usize, redact: Redactor) -> io::Result<NonBlocking> {
        let (tx, rx) = sync_channel(queue);
        let dropped = std::sync::Arc::new(AtomicU64::new(0));
        let d = dropped.clone();
        std::thread::Builder::new()
            .name("qrtz-log".into())
            .spawn(move || writer_thread(file, rx, d))?;
        Ok(NonBlocking { tx, dropped, redact })
    }

    fn send(&self, line: String) {
        let mut b = line.into_bytes();
        b.push(b'\n');
        match self.tx.try_send(Msg::Line(b)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Wait (up to `timeout`) until every line queued so far is in the file.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (ack, done) = sync_channel(1);
        let deadline = Instant::now() + timeout;
        let mut msg = Msg::Flush(ack);
        loop {
            match self.tx.try_send(msg) {
                Ok(()) => break,
                Err(TrySendError::Full(m)) if Instant::now() < deadline => {
                    msg = m;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return false,
            }
        }
        done.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
    }
}

fn writer_thread(mut file: RotatingFile, rx: Receiver<Msg>, dropped: std::sync::Arc<AtomicU64>) {
    while let Ok(m) = rx.recv() {
        match m {
            Msg::Line(b) => {
                let n = dropped.swap(0, Ordering::Relaxed);
                if n > 0 {
                    let note = format!(
                        "{} WARN qrtz_app::logging: {n} log lines dropped (queue full)\n",
                        now_utc()
                    );
                    let _ = file.write_line(note.as_bytes());
                }
                // A failing disk must not take the app down; the line is lost.
                let _ = file.write_line(&b);
            }
            Msg::Flush(ack) => {
                let _ = file.flush();
                let _ = ack.try_send(());
            }
        }
    }
    let _ = file.flush();
}

/// Seconds since the epoch as `…Z` (the drop note only; events use the fmt layer's timer).
fn now_utc() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("t={}.{:03}Z", d.as_secs(), d.subsec_millis())
}

/// One event's bytes, redacted and sent when the fmt layer drops the writer.
pub struct LineWriter<'a> {
    buf: Vec<u8>,
    sink: &'a NonBlocking,
}
impl Write for LineWriter<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for LineWriter<'_> {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let line = self.sink.redact.line(&String::from_utf8_lossy(&self.buf));
            self.sink.send(line);
        }
    }
}
impl<'a> MakeWriter<'a> for NonBlocking {
    type Writer = LineWriter<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        LineWriter {
            buf: Vec::with_capacity(256),
            sink: self,
        }
    }
}

/// Any `Write` behind the same redaction, one event per write (stderr in the app, a buffer in tests).
#[derive(Clone)]
pub struct Redacted<W> {
    out: W,
    redact: Redactor,
}
impl<W> Redacted<W> {
    pub fn new(out: W, redact: Redactor) -> Self {
        Self { out, redact }
    }
}
pub struct RedactedWriter<W: Write> {
    buf: Vec<u8>,
    out: W,
    redact: Redactor,
}
impl<W: Write> Write for RedactedWriter<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl<W: Write> Drop for RedactedWriter<W> {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let mut line = self.redact.line(&String::from_utf8_lossy(&self.buf));
            line.push('\n');
            let _ = self.out.write_all(line.as_bytes());
        }
    }
}
impl<'a, M> MakeWriter<'a> for Redacted<M>
where
    M: MakeWriter<'a>,
{
    type Writer = RedactedWriter<M::Writer>;
    fn make_writer(&'a self) -> Self::Writer {
        RedactedWriter {
            buf: Vec::with_capacity(256),
            out: self.out.make_writer(),
            redact: self.redact.clone(),
        }
    }
}

static SINK: OnceLock<NonBlocking> = OnceLock::new();
static DIR: OnceLock<PathBuf> = OnceLock::new();

/// What the app passes at launch.
pub struct LogOptions {
    /// `None`: stderr only (no writable log location).
    pub dir: Option<PathBuf>,
    pub release: bool,
    /// `QRTZ_LOG`.
    pub env_filter: Option<String>,
    pub version: String,
    /// Build flavour and id, e.g. `release 1a2b3c4` or `debug dev`.
    pub build: String,
}

/// Install the process-wide subscriber (stderr + the rotating file) and log the startup line. Call once, first.
/// A log folder that can't be created leaves stderr logging on and says why.
pub fn init(o: LogOptions) {
    let redact = Redactor::new(home_dir().as_deref());
    let filter = EnvFilter::try_new(filter_directives(o.env_filter.as_deref(), o.release))
        .unwrap_or_else(|_| EnvFilter::new(filter_directives(None, true)));
    let stderr = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(Redacted::new(io::stderr, redact.clone()));
    let mut file_err = None;
    let file = o.dir.as_ref().and_then(|d| {
        match RotatingFile::open(d, MAX_FILE_BYTES, ROTATED_FILES)
            .and_then(|f| NonBlocking::spawn(f, QUEUE_LINES, redact.clone()))
        {
            Ok(sink) => {
                let _ = SINK.set(sink.clone());
                let _ = DIR.set(d.clone());
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(sink),
                )
            }
            Err(e) => {
                file_err = Some(e);
                None
            }
        }
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file)
        .try_init();
    install_panic_hook();
    tracing::info!(
        version = %o.version,
        build = %o.build,
        os = %os_version(),
        arch = std::env::consts::ARCH,
        log_dir = %o.dir.as_deref().map(|d| d.display().to_string()).unwrap_or_else(|| "none".into()),
        "qrtz started"
    );
    if let Some(e) = file_err {
        tracing::warn!(kind = %e.kind(), "log file unavailable; logging to stderr only");
    }
}

/// The log folder `init` opened, if any.
pub fn dir() -> Option<&'static Path> {
    DIR.get().map(PathBuf::as_path)
}

/// Wait (bounded) until queued lines reach the file, e.g. before an export.
pub fn flush() {
    if let Some(s) = SINK.get() {
        s.flush(Duration::from_secs(1));
    }
}

/// A panic is logged by location only: its message can quote data (a `str` slicing panic prints the string).
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "?".into());
        let thread = std::thread::current().name().unwrap_or("?").to_owned();
        tracing::error!(at = %at, thread = %thread, "panic");
        flush();
        prev(info);
    }));
}

/// macOS product version (`SystemVersion.plist`, no process spawned), the Linux distribution's pretty name, or
/// the OS name.
pub fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(p) = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist") {
            let val = |key: &str| {
                let at = p.find(&format!("<key>{key}</key>"))?;
                let rest = &p[at..];
                let s = rest.find("<string>")? + "<string>".len();
                let e = rest[s..].find("</string>")?;
                Some(rest[s..s + e].trim().to_owned())
            };
            if let Some(v) = val("ProductVersion") {
                return match val("ProductBuildVersion") {
                    Some(b) => format!("macOS {v} ({b})"),
                    None => format!("macOS {v}"),
                };
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(r) = std::fs::read_to_string("/etc/os-release") {
            if let Some(l) = r.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")) {
                return cut_chars(l.trim_matches('"'), 80).to_owned();
            }
        }
    }
    std::env::consts::OS.to_owned()
}

macro_rules! at_level {
    ($level:expr, target: $t:expr, $($rest:tt)*) => {
        match $level {
            "error" => tracing::error!(target: $t, $($rest)*),
            "warn" => tracing::warn!(target: $t, $($rest)*),
            "debug" => tracing::debug!(target: $t, $($rest)*),
            "trace" => tracing::trace!(target: $t, $($rest)*),
            _ => tracing::info!(target: $t, $($rest)*),
        }
    };
}

/// A helper `log` protocol line (`{"type":"log","level","msg"}`) into the log, target `helper`.
pub fn helper_log(level: &str, msg: &str) {
    let msg = cut_chars(msg, HELPER_MESSAGE_MAX);
    at_level!(level, target: HELPER_TARGET, "{msg}");
}

/// `log:write`'s rate limit: [`RENDERER_LINES_PER_WINDOW`] lines per [`RENDERER_WINDOW`].
#[derive(Debug)]
pub struct RendererGate {
    window_start: Option<Instant>,
    count: u32,
    dropped: u64,
}

impl Default for RendererGate {
    fn default() -> Self {
        Self::new()
    }
}

impl RendererGate {
    pub const fn new() -> Self {
        Self {
            window_start: None,
            count: 0,
            dropped: 0,
        }
    }

    /// `Some(dropped)` = log this line (after noting `dropped` earlier lines, if any); `None` = drop it.
    pub fn admit(&mut self, now: Instant) -> Option<u64> {
        let fresh = self
            .window_start
            .is_none_or(|s| now.saturating_duration_since(s) >= RENDERER_WINDOW);
        if fresh {
            self.window_start = Some(now);
            self.count = 0;
        }
        if self.count >= RENDERER_LINES_PER_WINDOW {
            self.dropped += 1;
            return None;
        }
        self.count += 1;
        Some(std::mem::take(&mut self.dropped))
    }
}

/// A scope from the renderer is kept only if it is a short identifier (`[a-z0-9._:-]{1,64}`).
pub fn renderer_scope(scope: &str) -> &str {
    let ok = !scope.is_empty()
        && scope.len() <= 64
        && scope
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b':' | b'-'));
    if ok {
        scope
    } else {
        "invalid-scope"
    }
}

/// A `log:write` line into the log, target `renderer`, gated by `gate`. Returns whether it was logged.
pub fn renderer_log(gate: &Mutex<RendererGate>, level: &str, scope: &str, message: &str) -> bool {
    let admitted = gate
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .admit(Instant::now());
    let Some(dropped) = admitted else {
        return false;
    };
    if dropped > 0 {
        tracing::warn!(target: RENDERER_TARGET, dropped, "renderer log lines dropped (rate limit)");
    }
    let scope = renderer_scope(scope);
    let message = cut_chars(message, RENDERER_MESSAGE_MAX);
    at_level!(level, target: RENDERER_TARGET, scope, "{message}");
    true
}

/// Concatenate the log files (oldest first) into `dest` (created 0600, replaced if it exists), after `header`.
/// Returns (files, bytes written).
pub fn export_to(dir: &Path, dest: &Path, header: &str) -> io::Result<(usize, u64)> {
    let files = log_files(dir);
    let tmp = dest.with_extension("partial");
    let _ = std::fs::remove_file(&tmp);
    let mut out = io::BufWriter::new(owner_only_file(&tmp)?);
    let mut bytes = 0u64;
    out.write_all(header.as_bytes())?;
    bytes += header.len() as u64;
    for f in &files {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let sep = format!("\n===== {name} =====\n");
        out.write_all(sep.as_bytes())?;
        bytes += sep.len() as u64;
        bytes += io::copy(&mut File::open(f)?, &mut out)?;
    }
    let f = out.into_inner().map_err(|e| e.into_error())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, dest)?;
    Ok((files.len(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_per_platform() {
        let home = Path::new("/Users/jared");
        let none = |_: &str| None;
        assert_eq!(
            log_dir("macos", Some(home), none),
            Some(PathBuf::from("/Users/jared/Library/Logs/qrtz"))
        );
        assert_eq!(
            log_dir("linux", Some(Path::new("/home/j")), none),
            Some(PathBuf::from("/home/j/.local/state/qrtz/logs"))
        );
        let xdg = |k: &str| (k == "XDG_STATE_HOME").then(|| "/x".to_string());
        assert_eq!(log_dir("linux", None, xdg), Some(PathBuf::from("/x/qrtz/logs")));
        let profile = |k: &str| (k == "QRTZ_USER_DATA").then(|| "/tmp/p".to_string());
        assert_eq!(
            log_dir("macos", Some(home), profile),
            Some(PathBuf::from("/tmp/p/logs"))
        );
        assert_eq!(log_dir("macos", None, none), None);
    }

    #[test]
    fn release_builds_take_a_bare_level_only() {
        let base = "warn,qrtz=info,helper=info,renderer=info";
        assert_eq!(filter_directives(None, true), base);
        assert_eq!(filter_directives(None, false), base);
        assert_eq!(
            filter_directives(Some("DEBUG"), true),
            "warn,qrtz=debug,helper=debug,renderer=debug"
        );
        assert_eq!(
            filter_directives(Some("error"), true),
            "error,qrtz=error,helper=error,renderer=error"
        );
        assert_eq!(filter_directives(Some("off"), true), "off");
        // A directive that would open third-party targets (tauri's IPC traces carry payloads) is ignored.
        for knob in [
            "trace,tauri=trace",
            "tauri=trace",
            "wry=debug",
            "[{text}]=trace",
            "qrtz_talk=trace",
        ] {
            assert_eq!(filter_directives(Some(knob), true), base, "{knob}");
        }
        assert_eq!(
            filter_directives(Some("qrtz_talk=trace"), false),
            "qrtz_talk=trace"
        );
    }

    #[derive(Clone, Default)]
    struct Buf(std::sync::Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Buf {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    /// The release filter through a real subscriber: qrtz targets at info, third-party ones only from warn.
    fn capture(directives: &str, f: impl FnOnce()) -> String {
        let buf = Buf::default();
        let b = buf.clone();
        let sub = tracing_subscriber::registry()
            .with(EnvFilter::new(directives))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(Redacted::new(move || b.clone(), Redactor::default())),
            );
        tracing::subscriber::with_default(sub, f);
        buf.text()
    }

    #[test]
    fn the_default_filter_keeps_our_targets_and_quiets_third_party_ones() {
        let out = capture(&filter_directives(Some("trace,tauri=trace"), true), || {
            tracing::info!(target: "qrtz_app::service", "ours-info");
            tracing::debug!(target: "qrtz_app::service", "ours-debug");
            tracing::info!(target: "helper", "helper-info");
            tracing::info!(target: "renderer", "renderer-info");
            tracing::info!(target: "tauri::ipc", "theirs-info");
            tracing::trace!(target: "tauri::ipc", "theirs-trace");
            tracing::warn!(target: "tauri::ipc", "theirs-warn");
        });
        for want in ["ours-info", "helper-info", "renderer-info", "theirs-warn"] {
            assert!(out.contains(want), "{want}: {out}");
        }
        for not in ["ours-debug", "theirs-info", "theirs-trace"] {
            assert!(!out.contains(not), "{not}: {out}");
        }
        // One event, one line, even with a newline in the message.
        let out = capture("info", || tracing::info!(target: "qrtz_x", "a\nb"));
        assert_eq!(out.lines().count(), 1, "{out}");
    }

    #[test]
    fn redaction_homes_paths_escapes_controls_and_caps_lines() {
        let r = Redactor::new(Some(Path::new("/Users/jared")));
        assert_eq!(
            r.line("data dir data=/Users/jared/Library/Application Support/qrtz\n"),
            "data dir data=~/Library/Application Support/qrtz"
        );
        assert_eq!(r.line("home=/Users/jared"), "home=~");
        // A longer name that only starts with the home path is not touched.
        assert_eq!(r.line("/Users/jaredx/a"), "/Users/jaredx/a");
        assert_eq!(
            r.line("a\nb\r\u{1b}[31mc\u{202e}d\te"),
            "a\\nb\\r\\u{1b}[31mc\\u{202e}d\te"
        );
        let long = "x".repeat(MAX_LINE_BYTES + 100);
        let l = r.line(&long);
        assert!(l.starts_with(&"x".repeat(MAX_LINE_BYTES)));
        assert!(l.ends_with("…[cut 100 bytes]"), "{}", &l[l.len() - 30..]);
        assert_eq!(Redactor::new(Some(Path::new("/"))).line("/a/b"), "/a/b");
    }

    #[test]
    fn rotation_caps_the_folder_and_files_are_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        let mut f = RotatingFile::open(&dir, 1000, 3).unwrap();
        let line = [b'a'; 99].iter().copied().chain([b'\n']).collect::<Vec<u8>>();
        for _ in 0..200 {
            f.write_line(&line).unwrap();
        }
        f.flush().unwrap();
        let files = log_files(&dir);
        assert_eq!(
            files
                .iter()
                .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            ["qrtz.3.log", "qrtz.2.log", "qrtz.1.log", "qrtz.log"]
        );
        let total: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert!(total <= 4 * 1000, "{total}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for p in files {
                assert_eq!(
                    std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "{p:?}"
                );
            }
        }
    }

    #[test]
    fn a_full_queue_drops_lines_instead_of_blocking_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let f = RotatingFile::open(tmp.path(), MAX_FILE_BYTES, 1).unwrap();
        // A queue of one, flooded from this thread: sends never block; whatever didn't fit is counted.
        let sink = NonBlocking::spawn(f, 1, Redactor::default()).unwrap();
        let t = Instant::now();
        for i in 0..10_000 {
            sink.send(format!("line {i}"));
        }
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        sink.send("last".into());
        assert!(sink.flush(Duration::from_secs(5)));
        sink.send("after".into());
        assert!(sink.flush(Duration::from_secs(5)));
        let text = std::fs::read_to_string(tmp.path().join(LOG_FILE)).unwrap();
        let written = text.lines().filter(|l| l.starts_with("line ")).count();
        if written < 10_000 {
            assert!(text.contains("log lines dropped (queue full)"), "{written}");
        }
        assert!(text.contains("after"));
    }

    #[test]
    fn renderer_gate_limits_per_window_and_reports_drops() {
        let mut g = RendererGate::new();
        let t0 = Instant::now();
        for _ in 0..RENDERER_LINES_PER_WINDOW {
            assert_eq!(g.admit(t0), Some(0));
        }
        assert_eq!(g.admit(t0), None);
        assert_eq!(g.admit(t0 + Duration::from_secs(1)), None);
        assert_eq!(g.admit(t0 + RENDERER_WINDOW), Some(2));
        assert_eq!(g.admit(t0 + RENDERER_WINDOW), Some(0));
    }

    #[test]
    fn renderer_scopes_are_identifiers() {
        assert_eq!(renderer_scope("player"), "player");
        assert_eq!(renderer_scope("note.player:error"), "note.player:error");
        for bad in ["", "Player", "a b", "x\ny", &"a".repeat(65)] {
            assert_eq!(renderer_scope(bad), "invalid-scope", "{bad:?}");
        }
    }

    #[test]
    fn export_concatenates_oldest_first_into_an_owner_only_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        let mut f = RotatingFile::open(&dir, 20, 2).unwrap();
        for l in ["one 0123456789\n", "two 0123456789\n", "three 01234567\n"] {
            f.write_line(l.as_bytes()).unwrap();
        }
        f.flush().unwrap();
        let dest = tmp.path().join("out.log");
        std::fs::write(&dest, "old").unwrap();
        let (n, bytes) = export_to(&dir, &dest, "qrtz logs\n").unwrap();
        assert_eq!(n, 3);
        let text = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(bytes, text.len() as u64);
        let (a, b, c) = (
            text.find("one").unwrap(),
            text.find("two").unwrap(),
            text.find("three").unwrap(),
        );
        assert!(text.starts_with("qrtz logs\n") && a < b && b < c, "{text}");
        assert!(!tmp.path().join("out.partial").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn cut_chars_respects_char_boundaries() {
        assert_eq!(cut_chars("héllo", 2), "hé");
        assert_eq!(cut_chars("hi", 5), "hi");
    }
}
