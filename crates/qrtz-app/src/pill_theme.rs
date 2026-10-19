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

/// Write `theme` to `file` unless it already says so (tmp + rename, so the pill never reads half a word). Returns
/// whether the file changed.
pub fn write(file: &Path, theme: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(file).is_ok_and(|s| s.trim() == theme) {
        return Ok(false);
    }
    let dir = file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true).mode(0o600);
        std::io::Write::write_all(&mut o.open(&tmp)?, format!("{theme}\n").as_bytes())?;
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
pub fn follow(
    file: PathBuf,
    mut rx: tokio::sync::broadcast::Receiver<(String, Value)>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("pill-theme".into())
        .spawn(move || loop {
            use tokio::sync::broadcast::error::RecvError;
            match rx.blocking_recv() {
                Ok((event, cfg)) if event == "config:changed" => sync(&file, &cfg),
                Ok(_) | Err(RecvError::Lagged(_)) => {}
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
        let t = follow(f.clone(), events.subscribe());
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
}
