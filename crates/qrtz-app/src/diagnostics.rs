//! Audit P1-3: "export diagnostics". One text file the user saves and sends themselves: the local logs (ADR 0083)
//! plus macOS's own crash, hang and spin reports for qrtz and its helpers (`~/Library/Logs/DiagnosticReports`,
//! where macOS writes `qrtz-helper-<date>.ips` when the Swift helper crashes). Local only and user-started: it
//! reads files on this Mac and writes one file; nothing is sent anywhere.
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Report kinds macOS writes (`.ips` today; `.crash`, `.hang`, `.spin`, `.diag` on older systems or other tools).
const EXTENSIONS: &[&str] = &["ips", "crash", "hang", "spin", "diag"];
/// Only reports this recent.
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// At most this many reports, newest first, each cut to this many bytes.
pub const MAX_REPORTS: usize = 20;
pub const MAX_REPORT_BYTES: u64 = 2 * 1024 * 1024;

/// macOS's per-user crash report folders (`~/Library/Logs/DiagnosticReports` and its `Retired` subfolder).
pub fn default_report_dirs() -> Vec<PathBuf> {
    if !cfg!(target_os = "macos") {
        return vec![];
    }
    crate::logging::home_dir()
        .map(|h| {
            let d = h.join("Library/Logs/DiagnosticReports");
            vec![d.join("Retired"), d]
        })
        .unwrap_or_default()
}

/// A report of one of qrtz's processes: `qrtz`, `qrtz-app`, `qrtz-helper`, `qrtz-echo`, `qrtz-nano`, `qrtz-ctrl`
/// (any `qrtz` name followed by `-`, `_` or `.`), with a crash-report extension. Another app's report never
/// matches (`qrtzfoo`, `notqrtz-…`).
pub fn is_qrtz_report(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some((stem, ext)) = lower.rsplit_once('.') else {
        return false;
    };
    EXTENSIONS.contains(&ext)
        && stem
            .strip_prefix("qrtz")
            .and_then(|r| r.chars().next())
            .is_some_and(|c| matches!(c, '-' | '_' | '.'))
}

/// qrtz's reports in `dirs`, newest first, no older than [`MAX_AGE`], at most [`MAX_REPORTS`].
pub fn crash_reports(dirs: &[PathBuf], now: SystemTime) -> Vec<PathBuf> {
    let mut found: Vec<(SystemTime, PathBuf)> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|it| it.flatten())
        .filter(|e| e.file_name().to_str().is_some_and(is_qrtz_report))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let t = m.modified().ok()?;
            (m.is_file() && now.duration_since(t).unwrap_or_default() <= MAX_AGE).then(|| (t, e.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.truncate(MAX_REPORTS);
    found.into_iter().map(|(_, p)| p).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exported {
    pub logs: usize,
    pub crash_reports: usize,
    pub bytes: u64,
}

/// Writes `header`, every log file in `log_dir` (oldest first), then each crash report, into `dest` (0600, via a
/// `.partial` file renamed into place).
pub fn export_to(log_dir: &Path, report_dirs: &[PathBuf], dest: &Path, header: &str) -> io::Result<Exported> {
    let logs = crate::logging::log_files(log_dir);
    let reports = crash_reports(report_dirs, SystemTime::now());
    let tmp = dest.with_extension("partial");
    let _ = std::fs::remove_file(&tmp);
    let mut out = io::BufWriter::new(crate::logging::owner_only_file(&tmp)?);
    let mut bytes = 0u64;
    let mut put = |out: &mut io::BufWriter<std::fs::File>, s: &str| -> io::Result<()> {
        out.write_all(s.as_bytes())?;
        bytes += s.len() as u64;
        Ok(())
    };
    put(&mut out, header)?;
    put(
        &mut out,
        &format!(
            "{} log files, {} crash reports (last {} days)\n",
            logs.len(),
            reports.len(),
            MAX_AGE.as_secs() / 86400
        ),
    )?;
    let mut copied = 0u64;
    for f in &logs {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        put(&mut out, &format!("\n===== log: {name} =====\n"))?;
        copied += io::copy(&mut std::fs::File::open(f)?, &mut out)?;
    }
    for f in &reports {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        put(&mut out, &format!("\n===== crash report: {name} =====\n"))?;
        // A report that vanished or can't be read is noted, not fatal.
        match std::fs::File::open(f) {
            Ok(r) => {
                let size = r.metadata().map(|m| m.len()).unwrap_or(0);
                copied += io::copy(&mut io::Read::take(r, MAX_REPORT_BYTES), &mut out)?;
                if size > MAX_REPORT_BYTES {
                    put(
                        &mut out,
                        &format!("\n[cut at {MAX_REPORT_BYTES} of {size} bytes]\n"),
                    )?;
                }
            }
            Err(e) => put(&mut out, &format!("[couldn't read: {}]\n", e.kind()))?,
        }
    }
    let f = out.into_inner().map_err(|e| e.into_error())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, dest)?;
    Ok(Exported {
        logs: logs.len(),
        crash_reports: reports.len(),
        bytes: bytes + copied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_qrtz_reports_match() {
        for n in [
            "qrtz-helper-2026-10-18-171500.ips",
            "qrtz-2026-10-18-171500.ips",
            "qrtz-app-2026-10-18-171500.ips",
            "qrtz-echo_2026-10-18-171500_Jareds-Mac.crash",
            "qrtz-ctrl-2026-10-18-171500.hang",
            "QRTZ-NANO-2026.spin",
        ] {
            assert!(is_qrtz_report(n), "{n}");
        }
        for n in [
            "qrtzfoo-2026.ips",
            "notqrtz-2026.ips",
            "Safari-2026-10-18.ips",
            "qrtz-helper-2026.log",
            "qrtz",
            ".ips",
        ] {
            assert!(!is_qrtz_report(n), "{n}");
        }
    }

    #[test]
    fn the_bundle_has_the_logs_then_qrtz_reports_only() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        let reports = tmp.path().join("DiagnosticReports");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(logs.join("qrtz.log"), "a log line\n").unwrap();
        std::fs::write(
            reports.join("qrtz-helper-2026-10-18-171500.ips"),
            "{\"app_name\":\"qrtz-helper\"}\n",
        )
        .unwrap();
        std::fs::write(reports.join("Safari-2026-10-18-171500.ips"), "not ours").unwrap();
        let dest = tmp.path().join("out.txt");
        let e = export_to(
            &logs,
            &[reports.clone(), tmp.path().join("missing")],
            &dest,
            "qrtz test\n",
        )
        .unwrap();
        assert_eq!((e.logs, e.crash_reports), (1, 1));
        let text = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(e.bytes, text.len() as u64);
        assert!(text.starts_with("qrtz test\n"));
        assert!(text.contains("a log line"));
        assert!(text.contains("===== crash report: qrtz-helper-2026-10-18-171500.ips"));
        assert!(text.contains("\"app_name\":\"qrtz-helper\""));
        assert!(!text.contains("not ours"));
        assert!(!dest.with_extension("partial").exists());
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
    fn old_reports_are_left_out_and_the_count_is_capped() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..(MAX_REPORTS + 5) {
            std::fs::write(tmp.path().join(format!("qrtz-helper-{i:02}.ips")), "x").unwrap();
        }
        let dirs = [tmp.path().to_path_buf()];
        assert_eq!(crash_reports(&dirs, SystemTime::now()).len(), MAX_REPORTS);
        let later = SystemTime::now() + MAX_AGE + Duration::from_secs(60);
        assert!(crash_reports(&dirs, later).is_empty());
    }
}
