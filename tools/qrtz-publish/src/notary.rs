//! Before a notarization record is signed into the index, the publisher checks it against the code it describes
//! (Bozniak's 902bcab review, design call (c)). `scripts/modules/publish.sh` writes the record from notarytool's
//! answer, but the spec is only a file: a record copied from another build, or code re-signed after it was
//! notarized, must not reach the index. So `qrtz-publish build` recomputes, on macOS, the cdhash of each package's
//! code (the extension's entry, or a service bundle's main executable) with `codesign -dvvv` and its sha256, and
//! refuses a record that names anything else. With `QRTZ_NOTARY_PROFILE` set it also reads `notarytool log
//! <submission>` and requires Apple's own answer: that submission, `Accepted`, and a ticket for that cdhash.
use crate::Result;
use qrtz_modules::index::{Notarization, NOTARIZATION_ACCEPTED};
use qrtz_modules::qrtz_json::PackageManifest;
use std::path::{Path, PathBuf};

/// The submission id of the FAKE records a dev index carries (`scripts/modules/build-packages.sh`,
/// [`crate::fake_notarization`]). Only `build --dev-index` takes it, and `--dev-index` takes nothing else.
pub const FAKE_SUBMISSION_ID: &str = "00000000-0000-4000-8000-000000000000";

/// Where the publisher reads what the code on disk really is.
pub trait CodeEvidence {
    /// The cdhash of `code` as it is signed on disk (40 lowercase hex).
    fn cdhash(&self, code: &Path) -> Result<String>;
    /// `notarytool log <submission_id>` (JSON), or None when no notary profile is configured.
    fn notary_log(&self, _submission_id: &str) -> Result<Option<String>> {
        Ok(None)
    }
}

/// How `build` treats the spec's notarization records.
pub enum Notarized<'a> {
    /// Taken as written: the library default for tests and dry runs (keys made at runtime, nothing uploaded). Never
    /// what the `qrtz-publish build` command does.
    Unchecked,
    /// `build --dev-index` (`bun run modules:dev`, debug builds only): every record must be a FAKE one
    /// ([`FAKE_SUBMISSION_ID`]); a real-looking record is refused rather than taken unchecked.
    DevIndex,
    /// `build`: every record is checked against the code ([`check_record`]).
    Checked(&'a dyn CodeEvidence),
}

/// The executable a package's notarization is about: an extension's `extension.entry`, or the main executable of a
/// service's bundle (`<bundle>/Contents/MacOS/<CFBundleExecutable>`; the one file there, as publish.sh picks it).
pub fn code_file(dir: &Path, m: &PackageManifest) -> Option<PathBuf> {
    match (&m.extension, &m.service) {
        (Some(e), _) => Some(dir.join(&e.entry)),
        (_, Some(s)) => {
            let macos = dir.join(&s.bundle).join("Contents/MacOS");
            let mut files: Vec<PathBuf> = std::fs::read_dir(&macos)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            files.sort();
            files.into_iter().next()
        }
        _ => None,
    }
}

/// The record `n` is about exactly `code`: the same sha256, the cdhash the code really has, and (when a notary
/// profile is configured) Apple's log for that submission says Accepted with a ticket for that cdhash.
pub fn check_record(code: &Path, n: &Notarization, ev: &dyn CodeEvidence) -> Result<()> {
    if n.submission_id == FAKE_SUBMISSION_ID {
        return Err(
            "a FAKE notarization record (a dev index's): publish real code through scripts/modules/publish.sh".into(),
        );
    }
    let bytes = std::fs::read(code).map_err(|e| format!("{}: {e}", code.display()))?;
    let sha = qrtz_modules::sha256_hex(&bytes);
    if sha != n.code_sha256 {
        return Err(format!(
            "{}: sha256 {sha} isn't the notarized file's {} (the code changed after notarization)",
            code.display(),
            n.code_sha256
        ));
    }
    let got = ev.cdhash(code)?.trim().to_ascii_lowercase();
    if got != n.cdhash {
        return Err(format!(
            "{}: cdhash {got} isn't the notarization record's {} (submission {}): re-notarize this build",
            code.display(),
            n.cdhash,
            n.submission_id
        ));
    }
    if let Some(log) = ev.notary_log(&n.submission_id)? {
        check_notary_log(&log, n)?;
    }
    Ok(())
}

/// Apple's `notarytool log` JSON agrees with the record: same submission (`jobId`), `Accepted`, and a ticket
/// (`ticketContents[].cdhash`) for the record's cdhash.
pub fn check_notary_log(log: &str, n: &Notarization) -> Result<()> {
    let v: serde_json::Value =
        serde_json::from_str(log).map_err(|e| format!("notarytool log isn't JSON: {e}"))?;
    let job = v["jobId"].as_str().unwrap_or_default();
    if !job.eq_ignore_ascii_case(&n.submission_id) {
        return Err(format!(
            "notarytool log is for submission {job:?}, not {}",
            n.submission_id
        ));
    }
    let status = v["status"].as_str().unwrap_or_default();
    if status != NOTARIZATION_ACCEPTED {
        return Err(format!(
            "notarytool says submission {} is {status:?}, not Accepted",
            n.submission_id
        ));
    }
    let ticketed = v["ticketContents"].as_array().is_some_and(|t| {
        t.iter().any(|e| {
            e["cdhash"]
                .as_str()
                .is_some_and(|c| c.eq_ignore_ascii_case(&n.cdhash))
        })
    });
    if !ticketed {
        return Err(format!(
            "notarytool's ticket for submission {} has no cdhash {}",
            n.submission_id, n.cdhash
        ));
    }
    Ok(())
}

/// What the machine says: `codesign -dvvv` for the cdhash, `xcrun notarytool log` with `notary_profile` (a
/// `notarytool store-credentials` keychain profile) for Apple's log. macOS only; elsewhere every check fails, so
/// code is never published from a machine that can't read its signature.
pub struct SystemEvidence {
    pub notary_profile: Option<String>,
}

impl SystemEvidence {
    /// `QRTZ_NOTARY_PROFILE` (publish.sh sets it to the profile it notarized with), if set.
    pub fn from_env() -> Self {
        Self {
            notary_profile: std::env::var("QRTZ_NOTARY_PROFILE")
                .ok()
                .filter(|p| !p.trim().is_empty()),
        }
    }
}

#[cfg(target_os = "macos")]
fn run(program: &str, args: &[&str]) -> Result<String> {
    let o = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "{program} {}: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    // codesign -d writes its report to stderr.
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    ))
}

/// `CDHash=<hex>` from a `codesign -dvvv` report.
pub fn parse_cdhash(report: &str) -> Option<String> {
    report
        .lines()
        .find_map(|l| l.trim().strip_prefix("CDHash="))
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

impl CodeEvidence for SystemEvidence {
    #[cfg(target_os = "macos")]
    fn cdhash(&self, code: &Path) -> Result<String> {
        let p = code.to_str().ok_or("path is not utf-8")?;
        let report = run("/usr/bin/codesign", &["-d", "-vvv", p])?;
        parse_cdhash(&report).ok_or_else(|| format!("{p}: codesign reports no CDHash (unsigned?)"))
    }

    #[cfg(not(target_os = "macos"))]
    fn cdhash(&self, code: &Path) -> Result<String> {
        Err(format!(
            "{}: reading a cdhash needs macOS (codesign): publish code from the Mac that notarized it",
            code.display()
        ))
    }

    #[cfg(target_os = "macos")]
    fn notary_log(&self, submission_id: &str) -> Result<Option<String>> {
        let Some(profile) = &self.notary_profile else {
            return Ok(None);
        };
        let o = std::process::Command::new("/usr/bin/xcrun")
            .args(["notarytool", "log", submission_id, "--keychain-profile", profile])
            .output()
            .map_err(|e| format!("notarytool: {e}"))?;
        if !o.status.success() {
            return Err(format!(
                "notarytool log {submission_id}: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Ok(Some(String::from_utf8_lossy(&o.stdout).into_owned()))
    }

    #[cfg(not(target_os = "macos"))]
    fn notary_log(&self, _submission_id: &str) -> Result<Option<String>> {
        match &self.notary_profile {
            Some(_) => Err("notarytool needs macOS".into()),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cdhash_comes_from_codesigns_report() {
        let report = "Executable=/x/bin/qrtz-echo\nIdentifier=ai.qrtz.echo\nCodeDirectory v=20500 size=1 \
                      flags=0x10000(runtime)\nHash type=sha256 size=32\nCDHash=ABCDEF0123456789abcdef0123456789ABCDEF01\n";
        assert_eq!(
            parse_cdhash(report).as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef01")
        );
        assert_eq!(parse_cdhash("CDHash=xyz\n"), None);
        assert_eq!(parse_cdhash("Identifier=a\n"), None);
    }
}
