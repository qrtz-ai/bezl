//! ADR 0211: echo's native pill draws in qrtz's own theme setting (`theme`: dark, light or system), not in whatever
//! macOS uses. The host keeps the setting as one word in a file of its own, `<data>/pill/theme` (folder 0700, file
//! 0600, replaced by a rename), written before the helper starts and after every `config:changed`; the helper is
//! told the path in [`ENV`] and its pill reads the file when it is built and again whenever the folder changes
//! (native/qrtz-helper QrtzEcho `PillTheme`, QrtzEchoMac `PillThemeWatch`). No helper-protocol message and nothing
//! on the dictation path.
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Names the theme file for the helper (and the echo app the store installs: it inherits the helper's env).
pub const ENV: &str = "QRTZ_PILL_THEME_FILE";

/// `<data>/pill/theme`.
pub fn path(data: &Path) -> PathBuf {
    data.join("pill").join("theme")
}

/// The pill's theme for a config: the setting's value, or the setting's default (dark) for anything else.
pub fn theme_of(cfg: &Value) -> &'static str {
    match cfg["theme"].as_str() {
        Some("light") => "light",
        Some("system") => "system",
        _ => "dark",
    }
}

/// The file holds one word; anything longer is not a theme (the pill's `PillTheme.maxBytes`).
const MAX_BYTES: usize = 16;

/// What `file` says now, read the way the pill reads it: never through a symlink, never blocking on a FIFO, only
/// a regular file, at most [`MAX_BYTES`]. `None` for anything else (then the file is rewritten).
fn current(file: &Path) -> Option<String> {
    use std::io::Read;
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let f = o.open(file).ok()?;
    if !f.metadata().ok()?.file_type().is_file() {
        return None;
    }
    let mut buf = Vec::with_capacity(MAX_BYTES + 1);
    f.take(MAX_BYTES as u64 + 1).read_to_end(&mut buf).ok()?;
    if buf.len() > MAX_BYTES {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// Create `tmp` fresh (never following a symlink planted there); a leftover is removed and the create retried once.
#[cfg(unix)]
fn create_tmp(tmp: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let open = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(tmp)
    };
    match open() {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(tmp)?;
            open()
        }
        r => r,
    }
}

/// Write `theme` to `file` unless it already says so (tmp + rename, so the pill never reads half a word). Returns
/// whether the file changed. The folder must be a real directory (a symlink or anything else is refused).
pub fn write(file: &Path, theme: &str) -> std::io::Result<bool> {
    if current(file).is_some_and(|s| s.trim() == theme) {
        return Ok(false);
    }
    let dir = file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    if !std::fs::symlink_metadata(dir)?.file_type().is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        std::io::Write::write_all(&mut create_tmp(&tmp)?, format!("{theme}\n").as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, format!("{theme}\n"))?;
    std::fs::rename(&tmp, file)?;
    Ok(true)
}

/// Write the theme for `cfg`, logging (never failing) when the file can't be written: the pill then keeps the
/// theme it has.
pub fn sync(file: &Path, cfg: &Value) {
    if let Err(e) = write(file, theme_of(cfg)) {
        tracing::warn!(error = %e, "pill theme not saved; echo's pill keeps its theme");
    }
}

/// Follow `config:changed` on its own thread until the app's events end, rewriting the file when the theme changes.
/// Events missed (`Lagged`) re-sync from `config_now`, the current config.
pub fn follow(
    file: PathBuf,
    mut rx: tokio::sync::broadcast::Receiver<(String, Value)>,
    config_now: impl Fn() -> Value + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("pill-theme".into())
        .spawn(move || loop {
            use tokio::sync::broadcast::error::RecvError;
            match rx.blocking_recv() {
                Ok((event, cfg)) if event == "config:changed" => sync(&file, &cfg),
                Err(RecvError::Lagged(_)) => sync(&file, &config_now()),
                Ok(_) => {}
                Err(RecvError::Closed) => break,
            }
        })
        .expect("spawn pill-theme thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_theme_is_the_setting_or_its_default() {
        assert_eq!(theme_of(&json!({"theme": "light"})), "light");
        assert_eq!(theme_of(&json!({"theme": "system"})), "system");
        assert_eq!(theme_of(&json!({"theme": "dark"})), "dark");
        assert_eq!(theme_of(&json!({})), "dark", "the setting's default");
        assert_eq!(theme_of(&json!({"theme": "darkAqua"})), "dark");
    }

    #[test]
    fn written_once_owner_only_and_only_when_it_changes() {
        let d = tempfile::tempdir().unwrap();
        let f = path(d.path());
        assert!(write(&f, "light").unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "light\n");
        assert!(!write(&f, "light").unwrap(), "unchanged: not rewritten");
        assert!(write(&f, "system").unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "system\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&f), 0o600);
            assert_eq!(mode(f.parent().unwrap()), 0o700);
        }
        // No temp file left next to it.
        let names: Vec<_> = std::fs::read_dir(f.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("theme")]);
    }

    #[test]
    fn follows_config_changed_live() {
        let d = tempfile::tempdir().unwrap();
        let f = path(d.path());
        let events = crate::service::Events::new();
        sync(&f, &json!({"theme": "dark"}));
        let t = follow(f.clone(), events.subscribe(), || json!({}));
        events.emit("config:changed", json!({"theme": "light"}));
        events.emit("helper:status", json!({"state": "running"}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_to_string(&f).unwrap() != "light\n" {
            assert!(
                std::time::Instant::now() < deadline,
                "the file follows the setting"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(events);
        t.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_or_fifo_theme_file_is_never_read_and_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let f = path(d.path());
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        // A symlink to a file that already says "light": not trusted, replaced by a regular file.
        let target = d.path().join("elsewhere");
        std::fs::write(&target, "light\n").unwrap();
        std::os::unix::fs::symlink(&target, &f).unwrap();
        assert_eq!(current(&f), None);
        assert!(write(&f, "light").unwrap());
        assert!(std::fs::symlink_metadata(&f).unwrap().file_type().is_file());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "light\n",
            "target untouched"
        );
        // A FIFO: the check neither blocks nor reads it.
        std::fs::remove_file(&f).unwrap();
        let c = std::ffi::CString::new(f.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(current(&f), None);
        assert!(write(&f, "dark").unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "dark\n");
        // Too long: not a theme.
        std::fs::write(&f, format!("{}dark", " ".repeat(20))).unwrap();
        assert_eq!(current(&f), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_pill_dir_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, d.path().join("pill")).unwrap();
        let f = path(d.path());
        assert!(write(&f, "light").is_err());
        assert_eq!(
            std::fs::read_dir(&real).unwrap().count(),
            0,
            "nothing written through it"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_tmp_symlink_is_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let f = path(d.path());
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        let victim = d.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        let tmp = f.with_extension(format!("tmp-{}", std::process::id()));
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();
        assert!(write(&f, "system").unwrap());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "system\n");
    }

    #[test]
    fn a_lagged_follower_resyncs_from_the_current_config() {
        let d = tempfile::tempdir().unwrap();
        let f = path(d.path());
        sync(&f, &json!({"theme": "dark"}));
        let (tx, rx) = tokio::sync::broadcast::channel::<(String, Value)>(1);
        // Overflow the channel before the follower reads: it sees Lagged first.
        for _ in 0..4 {
            tx.send(("helper:status".into(), json!({}))).unwrap();
        }
        let t = follow(f.clone(), rx, || json!({"theme": "light"}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_to_string(&f).unwrap() != "light\n" {
            assert!(std::time::Instant::now() < deadline, "re-synced after Lagged");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(tx);
        t.join().unwrap();
    }
}
