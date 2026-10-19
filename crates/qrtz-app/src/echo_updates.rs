//! PROPOSAL (branch `proposal/echo-independent-updates`, not accepted; ADR 0174 stands on main): echo updates on
//! its own, as its own signed app, without replacing qrtz.app. Written against Bozniak's requirements (ADR 0181,
//! "Proposal (not accepted)"); every rule below names the requirement it answers.
//!
//! On disk (`<data>/apps/echo/`, user-writable, so nothing here is trusted until checked):
//! - `versions/<install id>/qrtz-echo.app`: one fresh dir per install, never written again (R4);
//! - `current`, `previous`: one line each naming an install dir; replaced by `rename` only (atomic pointer swap, R4);
//! - `high-water`: the highest echo version ever accepted; raised before the swap, never lowered (R3, B2).
//!
//! Which echo runs ([`select`]): the copies `current` and `previous` point at, ordered by the `CFBundleVersion` inside
//! each signed bundle (never the dir name, the pointer or the lock; B1). A copy runs only if its version is at least
//! the high-water mark and newer than the echo bundled in qrtz.app (R5, B2), and it passes [`BundleCheck`]: Developer
//! ID, team MR26CCZ496, identifier `ai.qrtz.echo`, the requirement built from that name and team at compile time
//! ([`crate::installed::launch_requirement`], R1), notarized, entitlements microphone-only (R6). Otherwise the bundled
//! echo runs: a log line and `modules.echo.bundledFallback = true` say so (B5). Whatever is picked, the spawn starts it
//! suspended and checks the running code by pid against the same requirement before `SIGCONT` (R2, B3;
//! `InstalledSpawn::spawn`, unchanged). Parakeet is not part of this: its model stays pinned by hash on its own (R7).
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const ECHO_BUNDLE: &str = "qrtz-echo.app";
pub const ECHO_EXE: &str = "Contents/MacOS/qrtz-echo";
pub const ECHO_IDENTIFIER: &str = "ai.qrtz.echo";
/// The only entitlement echo's own bundle may carry (R6; mac-gate checks the same list).
pub const ECHO_ENTITLEMENTS: [&str; 1] = ["com.apple.security.device.audio-input"];

/// `<data>/apps/echo`.
pub fn echo_root(data: &Path) -> PathBuf {
    data.join("apps").join("echo")
}

/// The checks a copy of echo must pass before it may be picked or installed (R1, R6). Release builds use
/// [`MacBundleCheck`]; tests use a fake.
pub trait BundleCheck: Send + Sync {
    fn check(&self, bundle: &Path, requirement: &str) -> Result<(), String>;
}

/// An index entry for echo's own package (the signed modules index, ADR 0169): what the installer checks the
/// download against (B4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub version: String,
    pub sha256: String,
    pub size: u64,
}

/// Which echo a launch runs, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// A downloaded copy: its bundle and the executable in it, and the version its signed Info.plist says.
    Installed {
        bundle: PathBuf,
        program: PathBuf,
        version: String,
    },
    /// The echo in qrtz.app. `why` is set when a downloaded copy existed but was refused (B5: the notice).
    Bundled { why: Option<String> },
}

impl Pick {
    pub fn is_fallback(&self) -> bool {
        matches!(self, Pick::Bundled { why: Some(_) })
    }
}

/// `semver(a) > semver(b)`; anything unparsable is never newer.
fn newer(a: &str, b: &str) -> bool {
    qrtz_modules::install::is_newer(a, b)
}

fn at_least(a: &str, b: &str) -> bool {
    a == b || newer(a, b)
}

/// The highest echo version ever accepted here, or None (no file, or one that doesn't parse: it counts as none,
/// and the bundled version is still a floor).
pub fn high_water(root: &Path) -> Option<String> {
    let s = std::fs::read_to_string(root.join("high-water")).ok()?;
    let v = s.trim();
    semver::Version::parse(v).ok().map(|_| v.to_owned())
}

/// Raise the mark to `v` (tmp + rename); never lowers it.
fn raise_high_water(root: &Path, v: &str) -> std::io::Result<()> {
    if high_water(root).is_some_and(|hw| at_least(&hw, v)) {
        return Ok(());
    }
    write_atomic(&root.join("high-water"), v)
}

fn write_atomic(p: &Path, text: &str) -> std::io::Result<()> {
    let tmp = p.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, format!("{text}\n"))?;
    std::fs::rename(&tmp, p)
}

fn pointer(root: &Path, name: &str) -> Option<PathBuf> {
    let s = std::fs::read_to_string(root.join(name)).ok()?;
    let id = s.trim();
    // A pointer names one plain dir under versions/ (no `/`, no `..`).
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return None;
    }
    Some(root.join("versions").join(id).join(ECHO_BUNDLE))
}

/// `(CFBundleIdentifier, CFBundleVersion)` from `bundle/Contents/Info.plist`. Read before the signature is checked
/// only to order candidates; the bundle check (which seals Info.plist) runs before one is used.
pub fn bundle_info(bundle: &Path) -> Result<(String, String), String> {
    let plist = bundle.join("Contents/Info.plist");
    let bytes = std::fs::read(&plist).map_err(|e| format!("{}: {e}", plist.display()))?;
    if bytes.starts_with(b"bplist") {
        return binary_plist_info(&plist);
    }
    let text = String::from_utf8_lossy(&bytes);
    let get = |k: &str| xml_plist_string(&text, k).ok_or_else(|| format!("Info.plist has no {k}"));
    Ok((get("CFBundleIdentifier")?, get("CFBundleVersion")?))
}

fn xml_plist_string(text: &str, key: &str) -> Option<String> {
    let at = text.find(&format!("<key>{key}</key>"))?;
    let rest = text[at..].split_once("</key>")?.1.trim_start();
    let v = rest.strip_prefix("<string>")?.split_once("</string>")?.0;
    Some(v.trim().to_owned())
}

#[cfg(target_os = "macos")]
fn binary_plist_info(plist: &Path) -> Result<(String, String), String> {
    let p = plist.to_str().ok_or("Info.plist path isn't utf-8")?;
    let get = |k: &str| {
        let o = std::process::Command::new("/usr/bin/plutil")
            .args(["-extract", k, "raw", "-o", "-", p])
            .output()
            .map_err(|e| format!("plutil: {e}"))?;
        if !o.status.success() {
            return Err(format!("Info.plist has no {k}"));
        }
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    Ok((get("CFBundleIdentifier")?, get("CFBundleVersion")?))
}

#[cfg(not(target_os = "macos"))]
fn binary_plist_info(_plist: &Path) -> Result<(String, String), String> {
    Err("binary Info.plist needs macOS".into())
}

/// The process-wide record of the last pick, which the host turns into `modules.echo.bundledFallback` (B5).
fn last_pick() -> &'static Mutex<Option<Pick>> {
    static LAST: OnceLock<Mutex<Option<Pick>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

/// Whether the last echo launch fell back to the bundled copy (None before any launch).
pub fn last_fallback() -> Option<bool> {
    last_pick()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(Pick::is_fallback)
}

/// The host's patch for `modules.echo.bundledFallback` after echo started (B5): `true` when the launch fell back to
/// the bundled echo, `false` when it didn't; None when config already says so. Only the host writes it (a renderer's
/// `config:set` touching it is refused, `installed::patch_touches_bundled_fallback`).
pub fn bundled_fallback_patch(config: &serde_json::Value, fallback: bool) -> Option<serde_json::Value> {
    let now = config["modules"]["echo"]["bundledFallback"]
        .as_bool()
        .unwrap_or(false);
    (now != fallback).then(|| serde_json::json!({"modules": {"echo": {"bundledFallback": fallback}}}))
}

/// Pick the echo to launch (see the module doc). `bundled_version` is the version of the echo inside qrtz.app.
pub fn select(root: &Path, bundled_version: &str, requirement: &str, check: &dyn BundleCheck) -> Pick {
    let floor = high_water(root);
    let mut tried = vec![];
    let mut candidates: Vec<(String, PathBuf)> = vec![];
    for name in ["current", "previous"] {
        let Some(bundle) = pointer(root, name) else {
            continue;
        };
        if candidates.iter().any(|(_, b)| *b == bundle) {
            continue;
        }
        match bundle_info(&bundle) {
            Ok((id, v)) if id == ECHO_IDENTIFIER => candidates.push((v, bundle)),
            Ok((id, _)) => tried.push(format!("{}: identifier {id}", bundle.display())),
            Err(e) => tried.push(e),
        }
    }
    // Newest first, by the version inside the bundle (B1).
    candidates.sort_by(|a, b| {
        let (x, y) = (
            semver::Version::parse(&a.0).ok(),
            semver::Version::parse(&b.0).ok(),
        );
        y.cmp(&x)
    });
    let mut pick = Pick::Bundled { why: None };
    for (v, bundle) in candidates {
        if !newer(&v, bundled_version) {
            // R5: a copy no newer than the bundled echo is ignored, quietly (an update of qrtz.app overtook it).
            continue;
        }
        if let Some(hw) = &floor {
            if !at_least(&v, hw) {
                tried.push(format!(
                    "echo {v} is below the highest version accepted here ({hw})"
                ));
                continue;
            }
        }
        match check.check(&bundle, requirement) {
            Ok(()) => {
                pick = Pick::Installed {
                    program: bundle.join(ECHO_EXE),
                    bundle,
                    version: v,
                };
                break;
            }
            Err(e) => tried.push(format!("echo {v}: {e}")),
        }
    }
    if matches!(pick, Pick::Bundled { .. }) && !tried.is_empty() {
        let why = tried.join("; ");
        tracing::warn!(target: "qrtz::echo", why = %why, "echo: running the copy bundled in qrtz.app");
        pick = Pick::Bundled { why: Some(why) };
    }
    *last_pick().lock().unwrap_or_else(|p| p.into_inner()) = Some(pick.clone());
    pick
}

/// Install a downloaded echo archive (a `.tar.gz` holding exactly `qrtz-echo.app`) as the new current copy.
/// In order: size and sha256 against the signed index entry (B4); the version must be newer than the high-water mark
/// and the bundled echo (R3, B2: a downgrade is refused here, not only at launch); unpack into a fresh install dir;
/// the bundle's own identifier and `CFBundleVersion` must be `ai.qrtz.echo` and the index version; [`BundleCheck`];
/// raise the mark; then swap the pointers (`previous` ← old `current`, `current` ← new). Dirs neither pointer names are
/// removed afterwards; the running copy is always one of the two, so it is never touched (R4).
pub fn install(
    root: &Path,
    archive: &Path,
    entry: &IndexEntry,
    bundled_version: &str,
    requirement: &str,
    check: &dyn BundleCheck,
) -> Result<PathBuf, String> {
    let (size, sha) = qrtz_modules::fsutil::sha256_file(archive).map_err(|e| format!("echo archive: {e}"))?;
    if size != entry.size || !sha.eq_ignore_ascii_case(&entry.sha256) {
        return Err("the echo download doesn't match the signed index".into());
    }
    let hw = high_water(root);
    if !newer(&entry.version, bundled_version) {
        return Err(format!(
            "echo {} is not newer than the echo in qrtz.app ({bundled_version})",
            entry.version
        ));
    }
    if let Some(hw) = &hw {
        if !newer(&entry.version, hw) {
            return Err(format!(
                "echo {} is not newer than the highest version accepted here ({hw})",
                entry.version
            ));
        }
    }
    let versions = root.join("versions");
    std::fs::create_dir_all(&versions).map_err(|e| format!("{}: {e}", versions.display()))?;
    let id = format!("{}-{}", entry.version, &sha[..16]);
    let dir = versions.join(&id);
    let staging = versions.join(format!(".staging-{id}-{}", std::process::id()));
    if dir.exists() {
        return Err(format!("{}: already installed", dir.display()));
    }
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let res = (|| {
        unpack(archive, &staging)?;
        let bundle = staging.join(ECHO_BUNDLE);
        let (bid, v) = bundle_info(&bundle)?;
        if bid != ECHO_IDENTIFIER || v != entry.version {
            return Err(format!(
                "the downloaded bundle is {bid} {v}, not {ECHO_IDENTIFIER} {}",
                entry.version
            ));
        }
        check.check(&bundle, requirement)?;
        std::fs::rename(&staging, &dir).map_err(|e| format!("{}: {e}", dir.display()))
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    raise_high_water(root, &entry.version).map_err(|e| format!("high-water: {e}"))?;
    if let Ok(cur) = std::fs::read_to_string(root.join("current")) {
        write_atomic(&root.join("previous"), cur.trim()).map_err(|e| format!("previous: {e}"))?;
    }
    write_atomic(&root.join("current"), &id).map_err(|e| format!("current: {e}"))?;
    prune(root);
    Ok(dir.join(ECHO_BUNDLE))
}

/// Unpack `archive` into the empty dir `into`; it must hold exactly `qrtz-echo.app`, as a dir.
fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    let o = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .map_err(|e| format!("tar: {e}"))?;
    if !o.status.success() {
        return Err(format!("tar: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let names: Vec<_> = std::fs::read_dir(into)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    let bundle = into.join(ECHO_BUNDLE);
    if names.len() != 1
        || names[0] != ECHO_BUNDLE
        || !std::fs::symlink_metadata(&bundle).is_ok_and(|m| m.is_dir())
    {
        return Err(format!("the echo archive must hold exactly {ECHO_BUNDLE}"));
    }
    Ok(())
}

fn prune(root: &Path) {
    let keep: Vec<String> = ["current", "previous"]
        .iter()
        .filter_map(|n| std::fs::read_to_string(root.join(n)).ok())
        .map(|s| s.trim().to_owned())
        .collect();
    let Ok(rd) = std::fs::read_dir(root.join("versions")) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.starts_with('.') && !keep.contains(&n) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Release check of a copy of echo's bundle (R1, R6): `codesign --verify --deep --strict -R=<requirement and
/// notarized>`, Gatekeeper's notarization verdict, and its entitlements exactly [`ECHO_ENTITLEMENTS`].
pub struct MacBundleCheck;

#[cfg(target_os = "macos")]
impl BundleCheck for MacBundleCheck {
    fn check(&self, bundle: &Path, requirement: &str) -> Result<(), String> {
        let p = bundle.to_str().ok_or("echo bundle path isn't utf-8")?;
        let run = |prog: &str, args: &[&str]| {
            std::process::Command::new(prog)
                .args(args)
                .output()
                .map_err(|e| format!("{prog}: {e}"))
        };
        let req = format!("-R={requirement} and notarized");
        let o = run("/usr/bin/codesign", &["--verify", "--deep", "--strict", &req, p])?;
        if !o.status.success() {
            return Err(format!("codesign: {}", String::from_utf8_lossy(&o.stderr).trim()));
        }
        let o = run("/usr/sbin/spctl", &["--assess", "--type", "execute", "-vv", p])?;
        let said = String::from_utf8_lossy(&o.stderr).into_owned();
        if !o.status.success() || !said.contains("Notarized Developer ID") {
            return Err(format!("gatekeeper: {}", said.trim()));
        }
        let o = run("/usr/bin/codesign", &["-d", "--entitlements", "-", "--xml", p])?;
        entitlements_are_mic_only(&String::from_utf8_lossy(&o.stdout))
    }
}

#[cfg(not(target_os = "macos"))]
impl BundleCheck for MacBundleCheck {
    fn check(&self, _bundle: &Path, _requirement: &str) -> Result<(), String> {
        Err("echo code checks need macOS".into())
    }
}

/// The entitlements plist (XML) grants exactly [`ECHO_ENTITLEMENTS`], nothing else true (R6).
pub fn entitlements_are_mic_only(xml: &str) -> Result<(), String> {
    let flat: String = xml.split_whitespace().collect();
    let mut granted = vec![];
    let mut rest = flat.as_str();
    while let Some(at) = rest.find("<key>") {
        let after = &rest[at + 5..];
        let Some((k, tail)) = after.split_once("</key>") else {
            break;
        };
        if !tail.starts_with("<false/>") {
            granted.push(k.to_owned());
        }
        rest = tail;
    }
    if granted.iter().map(String::as_str).eq(ECHO_ENTITLEMENTS) {
        Ok(())
    } else {
        Err(format!(
            "echo's entitlements must be the microphone only, not {granted:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQ: &str = "identifier \"ai.qrtz.echo\" and anchor apple generic";

    /// Accepts any bundle whose `Contents/MacOS/qrtz-echo` says `signed` (a stand-in for the code signature).
    struct Fake;
    impl BundleCheck for Fake {
        fn check(&self, bundle: &Path, requirement: &str) -> Result<(), String> {
            assert_eq!(requirement, REQ);
            match std::fs::read_to_string(bundle.join(ECHO_EXE)) {
                Ok(s) if s == "signed" => Ok(()),
                _ => Err("the signature doesn't verify".into()),
            }
        }
    }

    fn make_bundle(at: &Path, id: &str, version: &str, exe: &str) {
        std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        std::fs::write(
            at.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\"?>\n<plist><dict>\n  <key>CFBundleIdentifier</key>\n  <string>{id}</string>\n  \
                 <key>CFBundleVersion</key>\n  <string>{version}</string>\n</dict></plist>\n"
            ),
        )
        .unwrap();
        std::fs::write(at.join(ECHO_EXE), exe).unwrap();
    }

    fn archive(tmp: &Path, version: &str, exe: &str) -> (PathBuf, IndexEntry) {
        let src = tmp.join(format!("src-{version}-{exe}"));
        make_bundle(&src.join(ECHO_BUNDLE), ECHO_IDENTIFIER, version, exe);
        let out = tmp.join(format!("echo-{version}-{exe}.tar.gz"));
        let ok = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&out)
            .arg("-C")
            .arg(&src)
            .arg(ECHO_BUNDLE)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let (size, sha256) = qrtz_modules::fsutil::sha256_file(&out).unwrap();
        (
            out,
            IndexEntry {
                version: version.into(),
                sha256,
                size,
            },
        )
    }

    #[test]
    fn a_valid_newer_copy_is_picked_by_the_version_inside_its_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let root = echo_root(tmp.path());
        let (a, e) = archive(tmp.path(), "1.3.0", "signed");
        install(&root, &a, &e, "1.2.0", REQ, &Fake).unwrap();
        let (b, e2) = archive(tmp.path(), "1.4.0", "signed");
        install(&root, &b, &e2, "1.2.0", REQ, &Fake).unwrap();
        // Pointers swapped: current is 1.4.0, previous is the last good 1.3.0, both kept.
        let Pick::Installed { version, program, .. } = select(&root, "1.2.0", REQ, &Fake) else {
            panic!("expected the installed copy")
        };
        assert_eq!(version, "1.4.0");
        assert!(program.ends_with(ECHO_EXE));
        assert_eq!(high_water(&root).as_deref(), Some("1.4.0"));
        assert!(pointer(&root, "previous").unwrap().exists());
        // Swap the pointers by hand: the newest by CFBundleVersion still wins, not the pointer (B1).
        let cur = std::fs::read_to_string(root.join("current")).unwrap();
        let prev = std::fs::read_to_string(root.join("previous")).unwrap();
        std::fs::write(root.join("current"), prev).unwrap();
        std::fs::write(root.join("previous"), cur).unwrap();
        assert!(
            matches!(select(&root, "1.2.0", REQ, &Fake), Pick::Installed { version, .. } if version == "1.4.0")
        );
    }

    #[test]
    fn a_downgrade_is_refused_at_install_and_ignored_at_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = echo_root(tmp.path());
        let (a, e) = archive(tmp.path(), "1.4.0", "signed");
        install(&root, &a, &e, "1.2.0", REQ, &Fake).unwrap();
        let good = std::fs::read_to_string(root.join("current")).unwrap();
        for v in ["1.3.0", "1.4.0"] {
            let (b, e2) = archive(tmp.path(), v, "signed");
            let err = install(&root, &b, &e2, "1.2.0", REQ, &Fake).unwrap_err();
            assert!(err.contains("not newer"), "{err}");
        }
        // Not newer than the bundled echo: refused even with no mark.
        let fresh = echo_root(&tmp.path().join("fresh"));
        let (c, e3) = archive(tmp.path(), "1.1.0", "signed");
        assert!(install(&fresh, &c, &e3, "1.2.0", REQ, &Fake)
            .unwrap_err()
            .contains("qrtz.app"));
        // A copy planted below the mark (an old dir and pointer put back by hand) is ignored at launch.
        let old = root.join("versions/planted");
        make_bundle(&old.join(ECHO_BUNDLE), ECHO_IDENTIFIER, "1.3.0", "signed");
        std::fs::write(root.join("current"), "planted\n").unwrap();
        std::fs::write(root.join("previous"), "planted\n").unwrap();
        let p = select(&root, "1.2.0", REQ, &Fake);
        assert!(
            matches!(&p, Pick::Bundled { why: Some(w) } if w.contains("below the highest")),
            "{p:?}"
        );
        // The mark never goes down.
        raise_high_water(&root, "1.0.0").unwrap();
        assert_eq!(high_water(&root).as_deref(), Some("1.4.0"));
        // A bundled echo that overtook the download runs, quietly (no notice: nothing was refused).
        std::fs::write(root.join("current"), &good).unwrap();
        assert_eq!(select(&root, "2.0.0", REQ, &Fake), Pick::Bundled { why: None });
    }

    #[test]
    fn a_tampered_copy_falls_back_to_the_bundled_echo_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let root = echo_root(tmp.path());
        let (a, e) = archive(tmp.path(), "1.3.0", "signed");
        let bundle = install(&root, &a, &e, "1.2.0", REQ, &Fake).unwrap();
        std::fs::write(bundle.join(ECHO_EXE), "patched").unwrap();
        let p = select(&root, "1.2.0", REQ, &Fake);
        assert!(p.is_fallback(), "{p:?}");
        // An unsigned download never gets installed, and leaves nothing behind.
        let (b, e2) = archive(tmp.path(), "1.5.0", "unsigned");
        assert!(install(&root, &b, &e2, "1.2.0", REQ, &Fake).is_err());
        assert_eq!(high_water(&root).as_deref(), Some("1.3.0"));
        // A download that isn't what the signed index says is refused before it is unpacked.
        let (c, mut e3) = archive(tmp.path(), "1.6.0", "signed");
        e3.sha256 = "0".repeat(64);
        assert!(install(&root, &c, &e3, "1.2.0", REQ, &Fake)
            .unwrap_err()
            .contains("signed index"));
        let dirs = std::fs::read_dir(root.join("versions")).unwrap().count();
        assert_eq!(dirs, 1, "only the first install is on disk");
    }

    #[test]
    fn the_bundle_must_say_it_is_echo_at_the_index_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = echo_root(tmp.path());
        let (a, mut e) = archive(tmp.path(), "1.3.0", "signed");
        // The index says 1.9.0, the signed bundle says 1.3.0: the bundle's own version decides, so it's refused.
        e.version = "1.9.0".into();
        assert!(install(&root, &a, &e, "1.2.0", REQ, &Fake)
            .unwrap_err()
            .contains("not ai.qrtz.echo 1.9.0"));
        assert_eq!(high_water(&root), None);
    }

    #[test]
    fn the_host_sets_and_clears_bundled_fallback() {
        use serde_json::json;
        let none = json!({});
        assert_eq!(
            bundled_fallback_patch(&none, true),
            Some(json!({"modules": {"echo": {"bundledFallback": true}}}))
        );
        assert_eq!(bundled_fallback_patch(&none, false), None);
        let on = json!({"modules": {"echo": {"bundledFallback": true}}});
        assert_eq!(bundled_fallback_patch(&on, true), None);
        assert_eq!(
            bundled_fallback_patch(&on, false),
            Some(json!({"modules": {"echo": {"bundledFallback": false}}}))
        );
    }

    #[test]
    fn entitlements_must_be_the_microphone_only() {
        let mic = "<plist><dict><key>com.apple.security.device.audio-input</key><true/></dict></plist>";
        assert!(entitlements_are_mic_only(mic).is_ok());
        let off = "<plist><dict><key>com.apple.security.device.audio-input</key><true/>\
                   <key>com.apple.security.device.camera</key><false/></dict></plist>";
        assert!(entitlements_are_mic_only(off).is_ok());
        let more = "<plist><dict><key>com.apple.security.device.audio-input</key><true/>\
                    <key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>";
        assert!(entitlements_are_mic_only(more).is_err());
        assert!(entitlements_are_mic_only("<plist><dict/></plist>").is_err());
    }
}
