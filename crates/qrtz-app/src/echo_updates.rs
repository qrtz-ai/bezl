//! ADR 0185 (amends ADR 0174, Bozniak's review): echo updates on its own, as its own signed app, without replacing
//! qrtz.app. Every rule names the requirement it answers (R1–R7, B1–B5 in ADR 0185; E1–E4 and P3.1–P3.4 are his
//! review of the first draft).
//!
//! On disk (`<data>/apps/echo/`, user-writable, so nothing here is trusted until checked):
//! - `versions/<version>-<sha16>/qrtz-echo.app`: one fresh dir per install, never written again (R4);
//! - `current`, `previous`: one line each naming an install dir, replaced by `rename` only (R4);
//! - `high-water`: the highest echo version installed; raised after the pointers moved, never lowered (R3, E4). It is a
//!   plain file: a process running as the user can delete it, so the launch-time downgrade block is best-effort (E2,
//!   ADR 0185 says why a keychain item isn't better). What isn't best-effort: every copy that runs is a notarized
//!   `ai.qrtz.echo` of team MR26CCZ496 newer than the bundled echo, checked on the running process (E1);
//! - `bad`: install dirs that crashed [`MAX_CRASHES`] times in a row; never picked again (P3.2).
//!
//! Which echo runs ([`select`]): the copies `current` and `previous` name, each path checked once (no symlink
//! anywhere from `<data>/apps` down to the executable, real dirs and a plain file; E3) and that same path used for
//! the Info.plist read, the check and the spawn. They're ordered by the `CFBundleVersion` inside each bundle (never the
//! dir name, the pointer or the lock; B1). A copy runs only if it is at least the high-water mark and newer than the
//! echo bundled in qrtz.app (R5, B2), it passes [`BundleCheck::check_launch`] (offline and cheap, timed: the stapled
//! ticket, never the network; P3.1), and it hasn't crashed its way onto `bad`. Otherwise the bundled echo runs, and a
//! log line and `modules.echo.bundledFallback = true` say so (B5).
//!
//! The spawn (`InstalledSpawn::spawn`) starts the pick suspended and checks the running code by pid against the
//! compiled-in requirement; for a downloaded copy it then reads the running code's own signing information
//! (`SecCodeCopySigningInformation`) and requires `ai.qrtz.echo`, a `CFBundleVersion` at least [`Pick::Installed`]'s
//! `min_version` (the mark or the bundled version, whichever is higher) and microphone-only entitlements, or it
//! SIGKILLs it (R2, B3, E1). All of that runs on one `SecCode` for the pid, and the signing information's cdhash
//! (`kSecCodeInfoUnique`) is re-checked on it as `cdhash H"<unique>"`, so the Info.plist read from disk is the one
//! sealed by the running code (F1). Parakeet is not part of this: its model stays pinned by hash on its own (R7).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const ECHO_BUNDLE: &str = "qrtz-echo.app";
pub const ECHO_EXE: &str = "Contents/MacOS/qrtz-echo";
pub const ECHO_IDENTIFIER: &str = "ai.qrtz.echo";
/// The only entitlement echo's own bundle may carry (R6; mac-gate check e6 uses the same rule, P3.4).
pub const ECHO_ENTITLEMENTS: [&str; 1] = ["com.apple.security.device.audio-input"];
/// A downloaded echo that was started this many times in a row without running [`CRASH_WINDOW`] is set aside (P3.2).
pub const MAX_CRASHES: u32 = 3;
pub const CRASH_WINDOW: Duration = Duration::from_secs(10 * 60);
/// A launch check slower than this is logged (P3.1: it must stay cheap and offline).
pub const SLOW_LAUNCH_CHECK: Duration = Duration::from_millis(500);

/// `<data>/apps/echo`.
pub fn echo_root(data: &Path) -> PathBuf {
    data.join("apps").join("echo")
}

/// The checks a copy of echo's bundle must pass (R1, R6). Release builds use [`MacBundleCheck`]; tests use a fake.
pub trait BundleCheck: Send + Sync {
    /// Once, when installing: signature (deep, strict) against `requirement and notarized`, Gatekeeper's verdict,
    /// the stapled ticket (`stapler validate`), microphone-only entitlements.
    fn check_install(&self, bundle: &Path, requirement: &str) -> Result<(), String>;
    /// Every launch, before the spawn: offline and cheap (the stapled ticket must be there, so the `notarized` check
    /// never asks Apple), signature against `requirement and notarized`, microphone-only entitlements.
    fn check_launch(&self, bundle: &Path, requirement: &str) -> Result<(), String>;
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
    /// A downloaded copy: its install dir, its bundle and the executable in it (the one path checked, E3), the
    /// version its Info.plist says, and the lowest version the running process may report (E1).
    Installed {
        id: String,
        bundle: PathBuf,
        program: PathBuf,
        version: String,
        min_version: String,
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

/// The higher of two versions (`a` if `b` doesn't parse).
fn max_version(a: &str, b: Option<&str>) -> String {
    match b {
        Some(b) if newer(b, a) => b.to_owned(),
        _ => a.to_owned(),
    }
}

/// The highest echo version installed here, or None (no file, or one that doesn't parse: it counts as none, and the
/// bundled version is still a floor).
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

/// An install dir name: `<version>-<sha16>`, plain characters only.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'+')
}

fn pointer_id(root: &Path, name: &str) -> Option<String> {
    let s = std::fs::read_to_string(root.join(name)).ok()?;
    let id = s.trim();
    valid_id(id).then(|| id.to_owned())
}

fn real_dir(p: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Err(format!("{} is a symlink", p.display())),
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(format!("{} isn't a folder", p.display())),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// E3: the bundle of install `id`, checked once: `<data>/apps`, `apps/echo`, `versions`, `versions/<id>`,
/// `qrtz-echo.app`, `Contents` and `Contents/MacOS` are real folders (lstat, no symlink followed) and the executable a
/// plain file. The path returned is the one every later step uses.
pub fn resolve(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("{id:?} isn't an install folder name"));
    }
    let apps = root.parent().ok_or("no apps folder")?;
    let versions = root.join("versions");
    let dir = versions.join(id);
    let bundle = dir.join(ECHO_BUNDLE);
    for d in [
        apps.to_path_buf(),
        root.to_path_buf(),
        versions,
        dir,
        bundle.clone(),
        bundle.join("Contents"),
        bundle.join("Contents/MacOS"),
    ] {
        real_dir(&d)?;
    }
    let exe = bundle.join(ECHO_EXE);
    match std::fs::symlink_metadata(&exe) {
        Ok(m) if m.file_type().is_file() => Ok(bundle),
        Ok(_) => Err(format!("{} isn't a plain file", exe.display())),
        Err(e) => Err(format!("{}: {e}", exe.display())),
    }
}

/// `(CFBundleIdentifier, CFBundleVersion)` from `bundle/Contents/Info.plist`. Read before the signature is checked
/// only to order candidates; the bundle check (which seals Info.plist) runs before one is used, and the running
/// process's own Info.plist is checked again at spawn (E1).
pub fn bundle_info(bundle: &Path) -> Result<(String, String), String> {
    let plist = bundle.join("Contents/Info.plist");
    let m = std::fs::symlink_metadata(&plist).map_err(|e| format!("{}: {e}", plist.display()))?;
    if !m.file_type().is_file() {
        return Err(format!("{} isn't a plain file", plist.display()));
    }
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

/// The process-wide state: the last pick (the host turns it into `modules.echo.bundledFallback`, B5), the install dir
/// running now (never pruned, P3.3) and the launch counts per install dir (P3.2).
#[derive(Default)]
struct Shared {
    last: Option<Pick>,
    running: BTreeMap<PathBuf, String>,
    launches: BTreeMap<(PathBuf, String), Vec<Instant>>,
}

fn shared() -> std::sync::MutexGuard<'static, Shared> {
    static S: OnceLock<Mutex<Shared>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Shared::default()))
        .lock()
        .unwrap_or_else(crate::poison::recover)
}

/// Whether the last echo launch fell back to the bundled copy (None before any launch).
pub fn last_fallback() -> Option<bool> {
    shared().last.as_ref().map(Pick::is_fallback)
}

fn bad_ids(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("bad"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| valid_id(l))
        .collect()
}

/// P3.2: a downloaded echo (install `id` under `root`) was just started. Each start within [`CRASH_WINDOW`] of the
/// one before means the one before exited; after [`MAX_CRASHES`] of those it goes on `bad` and the next launch runs
/// the bundled echo. Also records it as the running copy (P3.3). Returns true when it was set aside.
pub fn launched(root: &Path, id: &str, now: Instant) -> bool {
    let crashed = {
        let mut s = shared();
        s.running.insert(root.to_path_buf(), id.to_owned());
        let t = s.launches.entry((root.to_path_buf(), id.to_owned())).or_default();
        if t.last().is_some_and(|l| now.duration_since(*l) > CRASH_WINDOW) {
            t.clear();
        }
        t.push(now);
        t.len() as u32 > MAX_CRASHES
    };
    if crashed && !bad_ids(root).iter().any(|b| b == id) {
        let mut all = bad_ids(root);
        all.push(id.to_owned());
        if let Err(e) = write_atomic(&root.join("bad"), &all.join("\n")) {
            tracing::warn!(target: "qrtz::echo", error = %e, "echo: couldn't set a crashing copy aside");
        }
        tracing::warn!(target: "qrtz::echo", id, "echo: the downloaded copy keeps crashing; running the bundled one");
    }
    crashed
}

/// Pick the echo to launch (see the module doc). `bundled_version` is the version of the echo inside qrtz.app.
pub fn select(root: &Path, bundled_version: &str, requirement: &str, check: &dyn BundleCheck) -> Pick {
    let mark = high_water(root);
    let min_version = max_version(bundled_version, mark.as_deref());
    let bad = bad_ids(root);
    let crashing: Vec<String> = {
        let s = shared();
        s.launches
            .iter()
            .filter(|((r, _), t)| r == root && t.len() as u32 > MAX_CRASHES)
            .map(|((_, id), _)| id.clone())
            .collect()
    };
    let mut tried = vec![];
    let mut candidates: Vec<(semver::Version, String, String, PathBuf)> = vec![];
    for name in ["current", "previous"] {
        let Some(id) = pointer_id(root, name) else {
            continue;
        };
        if candidates.iter().any(|c| c.2 == id) {
            continue;
        }
        if bad.contains(&id) || crashing.contains(&id) {
            tried.push(format!("{id}: set aside after crashing {MAX_CRASHES} times"));
            continue;
        }
        let bundle = match resolve(root, &id) {
            Ok(b) => b,
            Err(e) => {
                tried.push(e);
                continue;
            }
        };
        match bundle_info(&bundle) {
            Ok((bid, v)) if bid == ECHO_IDENTIFIER => match semver::Version::parse(&v) {
                Ok(sv) => candidates.push((sv, v, id, bundle)),
                Err(_) => tried.push(format!("{}: version {v:?}", bundle.display())),
            },
            Ok((bid, _)) => tried.push(format!("{}: identifier {bid}", bundle.display())),
            Err(e) => tried.push(e),
        }
    }
    // Newest first, by the version inside the bundle (B1).
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    let mut pick = Pick::Bundled { why: None };
    for (_, v, id, bundle) in candidates {
        if !newer(&v, bundled_version) {
            // R5: a copy no newer than the bundled echo is ignored, quietly (an update of qrtz.app overtook it).
            continue;
        }
        if !at_least(&v, &min_version) {
            tried.push(format!(
                "echo {v} is below the highest version installed here ({min_version})"
            ));
            continue;
        }
        let t = Instant::now();
        let res = check.check_launch(&bundle, requirement);
        let took = t.elapsed();
        if took > SLOW_LAUNCH_CHECK {
            tracing::warn!(target: "qrtz::echo", ms = took.as_millis() as u64, "echo: the launch check was slow");
        } else {
            tracing::debug!(target: "qrtz::echo", ms = took.as_millis() as u64, "echo: launch check");
        }
        match res {
            Ok(()) => {
                pick = Pick::Installed {
                    id,
                    program: bundle.join(ECHO_EXE),
                    bundle,
                    version: v,
                    min_version,
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
    shared().last = Some(pick.clone());
    pick
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

/// What the running process's own signature says (E1), read from its `SecCode` after the suspended spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningInfo {
    pub identifier: Option<String>,
    pub bundle_version: Option<String>,
    /// Entitlement keys whose value isn't `false` (the same rule as [`granted_entitlements`]).
    pub entitlements: Vec<String>,
}

/// E1: a downloaded echo's running process is `ai.qrtz.echo`, at least `min_version`, microphone-only.
pub fn check_running_info(info: &RunningInfo, min_version: &str) -> Result<(), String> {
    if info.identifier.as_deref() != Some(ECHO_IDENTIFIER) {
        return Err(format!(
            "the running code is {:?}, not {ECHO_IDENTIFIER}",
            info.identifier
        ));
    }
    let v = info
        .bundle_version
        .as_deref()
        .ok_or("the running code's Info.plist has no CFBundleVersion")?;
    if semver::Version::parse(v).is_err() || !at_least(v, min_version) {
        return Err(format!("the running echo is {v}, below {min_version}"));
    }
    let mut got = info.entitlements.clone();
    got.sort();
    if !got.iter().map(String::as_str).eq(ECHO_ENTITLEMENTS) {
        return Err(format!(
            "the running echo's entitlements must be the microphone only, not {got:?}"
        ));
    }
    Ok(())
}

/// F1: the code requirement that only the code with this cdhash (`kSecCodeInfoUnique`, 20 bytes) satisfies. A code
/// directory hash covers the Info.plist and entitlements slots, so code passing it is the code whose Info.plist and
/// entitlements [`running_info`] reported.
pub fn cdhash_requirement(unique: &[u8]) -> Result<String, String> {
    if unique.len() != 20 {
        return Err(format!(
            "the running code's cdhash is {} bytes, not 20",
            unique.len()
        ));
    }
    let hex: String = unique.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("cdhash H\"{hex}\""))
}

/// Install a downloaded echo archive (a `.tar.gz` holding exactly `qrtz-echo.app`) as the new current copy.
/// In order: size and sha256 against the signed index entry (B4); the version must be newer than the bundled echo
/// and the high-water mark (R3, B2), or equal to the mark when its install dir is already there (E4: a retry after an
/// interrupted install); unpack into a fresh staging dir, or reuse that dir; the bundle's identifier and
/// `CFBundleVersion` must be `ai.qrtz.echo` and the index version; [`BundleCheck::check_install`]; move it into place;
/// swap the pointers (`previous` ← old `current`, `current` ← new); only then raise the mark (E4), and prune.
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
    if !newer(&entry.version, bundled_version) {
        return Err(format!(
            "echo {} is not newer than the echo in qrtz.app ({bundled_version})",
            entry.version
        ));
    }
    let id = format!("{}-{}", entry.version, &sha[..16].to_ascii_lowercase());
    if !valid_id(&id) {
        return Err(format!("echo {}: not a plain version", entry.version));
    }
    let versions = root.join("versions");
    let dir = versions.join(&id);
    let exists = std::fs::symlink_metadata(&dir).is_ok();
    if let Some(hw) = high_water(root) {
        let retry = exists && entry.version == hw;
        if !newer(&entry.version, &hw) && !retry {
            return Err(format!(
                "echo {} is not newer than the highest version installed here ({hw})",
                entry.version
            ));
        }
    }
    std::fs::create_dir_all(&versions).map_err(|e| format!("{}: {e}", versions.display()))?;
    prune_staging(&versions);
    let check_bundle = |bundle: &Path| -> Result<(), String> {
        let (bid, v) = bundle_info(bundle)?;
        if bid != ECHO_IDENTIFIER || v != entry.version {
            return Err(format!(
                "the downloaded bundle is {bid} {v}, not {ECHO_IDENTIFIER} {}",
                entry.version
            ));
        }
        check.check_install(bundle, requirement)
    };
    if exists {
        // E4: the same version and bytes were installed before (an install interrupted before its mark, or a
        // retry): reuse the dir if it still checks out.
        let bundle = resolve(root, &id)?;
        check_bundle(&bundle).map_err(|e| format!("{}: {e}", dir.display()))?;
    } else {
        let staging = versions.join(format!(".staging-{id}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
        let res = (|| {
            unpack(archive, &staging)?;
            check_bundle(&staging.join(ECHO_BUNDLE))?;
            std::fs::rename(&staging, &dir).map_err(|e| format!("{}: {e}", dir.display()))
        })();
        if let Err(e) = res {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    // E4: pointers first, then the mark. A crash between the two leaves the new copy current with the old mark,
    // which launch accepts (it is above the mark); the reverse would leave a mark no pointer reaches.
    let cur = pointer_id(root, "current");
    if cur.as_deref() != Some(id.as_str()) {
        if let Some(cur) = &cur {
            write_atomic(&root.join("previous"), cur).map_err(|e| format!("previous: {e}"))?;
        }
        write_atomic(&root.join("current"), &id).map_err(|e| format!("current: {e}"))?;
    }
    raise_high_water(root, &entry.version).map_err(|e| format!("high-water: {e}"))?;
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
    if names.len() != 1 || names[0] != ECHO_BUNDLE || real_dir(&into.join(ECHO_BUNDLE)).is_err() {
        return Err(format!("the echo archive must hold exactly {ECHO_BUNDLE}"));
    }
    Ok(())
}

/// P3.3: leftover `.staging-*` dirs (an install that died midway) go.
fn prune_staging(versions: &Path) {
    let Ok(rd) = std::fs::read_dir(versions) else {
        return;
    };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with(".staging-") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Remove install dirs no pointer names, never the running one (P3.3), and staging leftovers.
fn prune(root: &Path) {
    let versions = root.join("versions");
    prune_staging(&versions);
    let mut keep: Vec<String> = ["current", "previous"]
        .iter()
        .filter_map(|n| pointer_id(root, n))
        .collect();
    if let Some(r) = shared().running.get(root) {
        keep.push(r.clone());
    }
    let Ok(rd) = std::fs::read_dir(&versions) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !keep.contains(&n) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// The entitlement keys an entitlements plist (XML, as `codesign -d --entitlements - --xml` prints it) grants: every
/// `<key>` whose value isn't `<false/>`, nested ones included (anything but a plain `false` counts as granted).
/// mac-gate's check e6 (`scripts/modules/entitlements.sh`) implements the same rule; both run on the vectors in
/// `spec/vectors/echo-entitlements/` (P3.4).
pub fn granted_entitlements(xml: &str) -> Vec<String> {
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
    granted.sort();
    granted
}

/// The entitlements plist grants exactly [`ECHO_ENTITLEMENTS`] (R6).
pub fn entitlements_are_mic_only(xml: &str) -> Result<(), String> {
    let granted = granted_entitlements(xml);
    if granted.iter().map(String::as_str).eq(ECHO_ENTITLEMENTS) {
        Ok(())
    } else {
        Err(format!(
            "echo's entitlements must be the microphone only, not {granted:?}"
        ))
    }
}

/// Release checks of a copy of echo's bundle (R1, R6, P3.1).
pub struct MacBundleCheck;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    pub(super) fn run(prog: &str, args: &[&str]) -> Result<std::process::Output, String> {
        std::process::Command::new(prog)
            .args(args)
            .output()
            .map_err(|e| format!("{prog}: {e}"))
    }

    pub(super) fn codesign(bundle: &str, requirement: &str, deep: bool) -> Result<(), String> {
        let req = format!("-R={requirement} and notarized");
        let mut args = vec!["--verify", "--strict"];
        if deep {
            args.push("--deep");
        }
        args.extend([req.as_str(), bundle]);
        let o = run("/usr/bin/codesign", &args)?;
        if o.status.success() {
            Ok(())
        } else {
            Err(format!("codesign: {}", String::from_utf8_lossy(&o.stderr).trim()))
        }
    }

    pub(super) fn entitlements(bundle: &str) -> Result<(), String> {
        let o = run(
            "/usr/bin/codesign",
            &["-d", "--entitlements", "-", "--xml", bundle],
        )?;
        entitlements_are_mic_only(&String::from_utf8_lossy(&o.stdout))
    }

    /// The notarization ticket is stapled into the bundle (`Contents/CodeResources`), so `notarized` is answered
    /// from it, offline.
    pub(super) fn ticket(bundle: &Path) -> Result<(), String> {
        let t = bundle.join("Contents/CodeResources");
        match std::fs::symlink_metadata(&t) {
            Ok(m) if m.file_type().is_file() && m.len() > 0 => Ok(()),
            _ => Err("no stapled notarization ticket".into()),
        }
    }

    /// `stapler validate`, from the developer tools if they're installed (never through the `xcrun` shim, which
    /// would offer to install them); otherwise the stapled-ticket check alone.
    pub(super) fn stapler(bundle: &str) -> Result<(), String> {
        for s in [
            "/Library/Developer/CommandLineTools/usr/bin/stapler",
            "/Applications/Xcode.app/Contents/Developer/usr/bin/stapler",
        ] {
            if Path::new(s).is_file() {
                let o = run(s, &["validate", "-q", bundle])?;
                return if o.status.success() {
                    Ok(())
                } else {
                    Err(format!("stapler: {}", String::from_utf8_lossy(&o.stdout).trim()))
                };
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl BundleCheck for MacBundleCheck {
    fn check_install(&self, bundle: &Path, requirement: &str) -> Result<(), String> {
        let p = bundle.to_str().ok_or("echo bundle path isn't utf-8")?;
        mac::ticket(bundle)?;
        mac::stapler(p)?;
        mac::codesign(p, requirement, true)?;
        let o = mac::run("/usr/sbin/spctl", &["--assess", "--type", "execute", "-vv", p])?;
        let said = String::from_utf8_lossy(&o.stderr).into_owned();
        let accepted = o.status.success() && said.contains("Notarized Developer ID");
        if !(accepted || said.contains("assessments disabled")) {
            return Err(format!("gatekeeper: {}", said.trim()));
        }
        mac::entitlements(p)
    }

    fn check_launch(&self, bundle: &Path, requirement: &str) -> Result<(), String> {
        let p = bundle.to_str().ok_or("echo bundle path isn't utf-8")?;
        // No Gatekeeper and no network: the stapled ticket must be there before codesign's `notarized` is asked.
        mac::ticket(bundle)?;
        mac::codesign(p, requirement, false)?;
        mac::entitlements(p)
    }
}

#[cfg(not(target_os = "macos"))]
impl BundleCheck for MacBundleCheck {
    fn check_install(&self, _bundle: &Path, _requirement: &str) -> Result<(), String> {
        Err("echo code checks need macOS".into())
    }
    fn check_launch(&self, _bundle: &Path, _requirement: &str) -> Result<(), String> {
        Err("echo code checks need macOS".into())
    }
}

/// E1/F1: what the running `code` (the suspended process's one `SecCode`, after `SecCodeCheckValidity` accepted it)
/// says about itself through `SecCodeCopySigningInformation`: its identifier, its Info.plist's `CFBundleVersion`, its
/// entitlements, and its cdhash (`kSecCodeInfoUnique`). The Info.plist comes from disk, so the caller re-checks the
/// same `code` against [`cdhash_requirement`] of that cdhash, which seals the Info.plist (F1).
#[cfg(target_os = "macos")]
pub fn running_info(
    code: &security_framework::os::macos::code_signing::SecCode,
) -> Result<(RunningInfo, Vec<u8>), String> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::data::CFData;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecCodeCopySigningInformation(
            code: *const std::ffi::c_void,
            flags: u32,
            information: *mut CFDictionaryRef,
        ) -> i32;
        static kSecCodeInfoIdentifier: CFStringRef;
        static kSecCodeInfoPList: CFStringRef;
        static kSecCodeInfoEntitlementsDict: CFStringRef;
        static kSecCodeInfoUnique: CFStringRef;
    }
    // kSecCSSigningInformation | kSecCSRequirementInformation (the entitlements dict comes with the latter).
    const FLAGS: u32 = (1 << 1) | (1 << 2);

    let mut info: CFDictionaryRef = std::ptr::null();
    // SAFETY: a valid SecCodeRef (SecCodeCopySigningInformation takes static or dynamic code) and an out pointer.
    let st = unsafe { SecCodeCopySigningInformation(code.as_CFTypeRef(), FLAGS, &mut info) };
    if st != 0 || info.is_null() {
        return Err(format!(
            "no signing information for the running code (OSStatus {st})"
        ));
    }
    // SAFETY: a +1 dictionary from a Copy function.
    let info: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(info) };
    // SAFETY: framework constants, valid for the process's life.
    let (k_id, k_plist, k_ent, k_unique) = unsafe {
        (
            CFString::wrap_under_get_rule(kSecCodeInfoIdentifier),
            CFString::wrap_under_get_rule(kSecCodeInfoPList),
            CFString::wrap_under_get_rule(kSecCodeInfoEntitlementsDict),
            CFString::wrap_under_get_rule(kSecCodeInfoUnique),
        )
    };
    let unique = info
        .find(&k_unique)
        .and_then(|v| v.downcast::<CFData>())
        .map(|d| d.bytes().to_vec())
        .ok_or("the running code's signing information has no cdhash")?;
    let string = |v: &CFType| v.downcast::<CFString>().map(|s| s.to_string());
    let identifier = info.find(&k_id).and_then(|v| string(&v));
    let bundle_version = info
        .find(&k_plist)
        .and_then(|v| v.downcast::<CFDictionary>())
        .and_then(|d| {
            // SAFETY: re-typing an untyped CFDictionary's keys and values as the CF types they are.
            let d: CFDictionary<CFString, CFType> =
                unsafe { CFDictionary::wrap_under_get_rule(d.as_concrete_TypeRef()) };
            d.find(CFString::new("CFBundleVersion")).and_then(|v| string(&v))
        });
    let mut entitlements = vec![];
    if let Some(d) = info.find(&k_ent).and_then(|v| v.downcast::<CFDictionary>()) {
        // SAFETY: as above.
        let d: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(d.as_concrete_TypeRef()) };
        let (keys, values) = d.get_keys_and_values();
        for (k, v) in keys.into_iter().zip(values) {
            // SAFETY: keys and values of a live dictionary, retained for the wrap.
            let (k, v) = unsafe {
                (
                    CFString::wrap_under_get_rule(k as CFStringRef),
                    CFType::wrap_under_get_rule(v),
                )
            };
            if v.downcast::<CFBoolean>().map(bool::from) != Some(false) {
                entitlements.push(k.to_string());
            }
        }
    }
    Ok((
        RunningInfo {
            identifier,
            bundle_version,
            entitlements,
        },
        unique,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQ: &str = "identifier \"ai.qrtz.echo\" and anchor apple generic";

    /// Accepts any bundle whose `Contents/MacOS/qrtz-echo` says `signed` (a stand-in for the code signature); counts
    /// the calls so a test can tell the install check from the launch check.
    #[derive(Default)]
    struct Fake {
        installs: Mutex<u32>,
        launches: Mutex<u32>,
    }
    impl Fake {
        fn verdict(bundle: &Path, requirement: &str) -> Result<(), String> {
            assert_eq!(requirement, REQ);
            match std::fs::read_to_string(bundle.join(ECHO_EXE)) {
                Ok(s) if s == "signed" => Ok(()),
                _ => Err("the signature doesn't verify".into()),
            }
        }
    }
    impl BundleCheck for Fake {
        fn check_install(&self, bundle: &Path, requirement: &str) -> Result<(), String> {
            *self.installs.lock().unwrap() += 1;
            Self::verdict(bundle, requirement)
        }
        fn check_launch(&self, bundle: &Path, requirement: &str) -> Result<(), String> {
            *self.launches.lock().unwrap() += 1;
            Self::verdict(bundle, requirement)
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

    fn root_in(tmp: &Path) -> PathBuf {
        echo_root(&tmp.join("data"))
    }

    fn current(root: &Path) -> String {
        pointer_id(root, "current").unwrap()
    }

    #[test]
    fn a_valid_newer_copy_is_picked_by_the_version_inside_its_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let root = root_in(tmp.path());
        let f = Fake::default();
        let (a, e) = archive(tmp.path(), "1.3.0", "signed");
        install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        let (b, e2) = archive(tmp.path(), "1.4.0", "signed");
        install(&root, &b, &e2, "1.2.0", REQ, &f).unwrap();
        let Pick::Installed {
            version,
            program,
            min_version,
            bundle,
            ..
        } = select(&root, "1.2.0", REQ, &f)
        else {
            panic!("expected the installed copy")
        };
        assert_eq!(version, "1.4.0");
        assert_eq!(min_version, "1.4.0", "the mark");
        assert_eq!(
            program,
            bundle.join(ECHO_EXE),
            "one path for the check and the spawn (E3)"
        );
        assert_eq!(high_water(&root).as_deref(), Some("1.4.0"));
        assert!(pointer_id(&root, "previous").is_some());
        // Swap the pointers by hand: the newest by CFBundleVersion still wins, not the pointer (B1).
        let (cur, prev) = (current(&root), pointer_id(&root, "previous").unwrap());
        std::fs::write(root.join("current"), &prev).unwrap();
        std::fs::write(root.join("previous"), &cur).unwrap();
        assert!(
            matches!(select(&root, "1.2.0", REQ, &f), Pick::Installed { version, .. } if version == "1.4.0")
        );
        // P3.1: launches use only the launch check; each install ran the install check once.
        assert_eq!(*f.installs.lock().unwrap(), 2);
        assert!(*f.launches.lock().unwrap() >= 2);
    }

    #[test]
    fn a_downgrade_is_refused_at_install_and_ignored_at_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = root_in(tmp.path());
        let f = Fake::default();
        let (a, e) = archive(tmp.path(), "1.4.0", "signed");
        install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        let good = current(&root);
        for v in ["1.3.0", "1.4.0"] {
            let (b, e2) = archive(tmp.path(), v, "different-bytes");
            let err = install(&root, &b, &e2, "1.2.0", REQ, &f).unwrap_err();
            assert!(err.contains("not newer"), "{v}: {err}");
        }
        // Not newer than the bundled echo: refused even with no mark.
        let fresh = root_in(&tmp.path().join("fresh"));
        let (c, e3) = archive(tmp.path(), "1.1.0", "signed");
        assert!(install(&fresh, &c, &e3, "1.2.0", REQ, &f)
            .unwrap_err()
            .contains("qrtz.app"));
        // A copy planted below the mark (an old dir and pointer put back by hand) is ignored at launch.
        let old = root.join("versions/1.3.0-planted");
        make_bundle(&old.join(ECHO_BUNDLE), ECHO_IDENTIFIER, "1.3.0", "signed");
        std::fs::write(root.join("current"), "1.3.0-planted\n").unwrap();
        std::fs::write(root.join("previous"), "1.3.0-planted\n").unwrap();
        let p = select(&root, "1.2.0", REQ, &f);
        assert!(
            matches!(&p, Pick::Bundled { why: Some(w) } if w.contains("below the highest")),
            "{p:?}"
        );
        // The mark never goes down.
        raise_high_water(&root, "1.0.0").unwrap();
        assert_eq!(high_water(&root).as_deref(), Some("1.4.0"));
        // A bundled echo that overtook the download runs, quietly (no notice: nothing was refused).
        std::fs::write(root.join("current"), &good).unwrap();
        assert_eq!(select(&root, "2.0.0", REQ, &f), Pick::Bundled { why: None });
    }

    #[test]
    fn a_tampered_copy_falls_back_to_the_bundled_echo_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let root = root_in(tmp.path());
        let f = Fake::default();
        let (a, e) = archive(tmp.path(), "1.3.0", "signed");
        let bundle = install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        std::fs::write(bundle.join(ECHO_EXE), "patched").unwrap();
        let p = select(&root, "1.2.0", REQ, &f);
        assert!(p.is_fallback(), "{p:?}");
        // An unsigned download never gets installed, and leaves nothing behind.
        let (b, e2) = archive(tmp.path(), "1.5.0", "unsigned");
        assert!(install(&root, &b, &e2, "1.2.0", REQ, &f).is_err());
        assert_eq!(high_water(&root).as_deref(), Some("1.3.0"));
        // A download that isn't what the signed index says is refused before it is unpacked.
        let (c, mut e3) = archive(tmp.path(), "1.6.0", "signed");
        e3.sha256 = "0".repeat(64);
        assert!(install(&root, &c, &e3, "1.2.0", REQ, &f)
            .unwrap_err()
            .contains("signed index"));
        let dirs = std::fs::read_dir(root.join("versions")).unwrap().count();
        assert_eq!(dirs, 1, "only the first install is on disk");
    }

    #[test]
    fn the_bundle_must_say_it_is_echo_at_the_index_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = root_in(tmp.path());
        let (a, mut e) = archive(tmp.path(), "1.3.0", "signed");
        // The index says 1.9.0, the signed bundle says 1.3.0: the bundle's own version decides, so it's refused.
        e.version = "1.9.0".into();
        assert!(install(&root, &a, &e, "1.2.0", REQ, &Fake::default())
            .unwrap_err()
            .contains("not ai.qrtz.echo 1.9.0"));
        assert_eq!(high_water(&root), None);
    }

    /// E3: no symlink anywhere from `<data>/apps` to the executable, at install dir, bundle, Contents or MacOS.
    #[cfg(unix)]
    #[test]
    fn symlinks_on_the_way_to_echo_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let root = root_in(tmp.path());
        let f = Fake::default();
        let (a, e) = archive(tmp.path(), "1.3.0", "signed");
        install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        let good = current(&root);
        assert!(resolve(&root, &good).is_ok());
        let elsewhere = tmp.path().join("elsewhere");
        make_bundle(&elsewhere.join(ECHO_BUNDLE), ECHO_IDENTIFIER, "1.9.0", "signed");
        let link = |target: &Path, at: &Path| {
            std::fs::remove_dir_all(at).ok();
            std::os::unix::fs::symlink(target, at).unwrap();
        };
        // The install dir, the bundle, Contents/MacOS and the executable, each a symlink in turn.
        let cases: Vec<(PathBuf, PathBuf)> = vec![
            (elsewhere.clone(), root.join("versions/1.9.0-link")),
            (
                elsewhere.join(ECHO_BUNDLE),
                root.join("versions/1.9.0-bundle").join(ECHO_BUNDLE),
            ),
            (
                elsewhere.join(ECHO_BUNDLE).join("Contents/MacOS"),
                root.join("versions/1.9.0-macos")
                    .join(ECHO_BUNDLE)
                    .join("Contents/MacOS"),
            ),
        ];
        for (target, at) in cases {
            if let Some(parent) = at.parent() {
                if at.ends_with("Contents/MacOS") {
                    make_bundle(parent.parent().unwrap(), ECHO_IDENTIFIER, "1.9.0", "signed");
                } else {
                    std::fs::create_dir_all(parent).unwrap();
                }
            }
            link(&target, &at);
            let id = at
                .strip_prefix(root.join("versions"))
                .unwrap()
                .components()
                .next()
                .unwrap()
                .as_os_str()
                .to_string_lossy()
                .into_owned();
            let err = resolve(&root, &id).unwrap_err();
            assert!(err.contains("symlink"), "{id}: {err}");
            std::fs::write(root.join("current"), &id).unwrap();
            std::fs::write(root.join("previous"), &good).unwrap();
            let p = select(&root, "1.2.0", REQ, &f);
            assert!(
                matches!(&p, Pick::Installed { version, .. } if version == "1.3.0"),
                "{id}: the linked copy is skipped, the real one runs: {p:?}"
            );
        }
        // An executable that is a symlink.
        let exe_case = root.join("versions/1.9.0-exe").join(ECHO_BUNDLE);
        make_bundle(&exe_case, ECHO_IDENTIFIER, "1.9.0", "signed");
        std::fs::remove_file(exe_case.join(ECHO_EXE)).unwrap();
        std::os::unix::fs::symlink(
            elsewhere.join(ECHO_BUNDLE).join(ECHO_EXE),
            exe_case.join(ECHO_EXE),
        )
        .unwrap();
        assert!(resolve(&root, "1.9.0-exe").unwrap_err().contains("plain file"));
        // A pointer that isn't a plain dir name.
        for bad in ["../x", ".staging-1", "a/b", ""] {
            assert!(resolve(&root, bad).is_err(), "{bad}");
        }
    }

    /// E4: the pointers move before the mark; a retry of the same version reuses its dir if it still checks out.
    #[test]
    fn an_interrupted_install_can_be_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let root = root_in(tmp.path());
        let f = Fake::default();
        let (a, e) = archive(tmp.path(), "1.3.0", "signed");
        install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        // Say the last install stopped after the pointers moved, before the mark (what E4's order allows): the mark
        // is behind. Launch still runs the new copy (it is above the mark), and the retry finishes the job.
        std::fs::write(root.join("high-water"), "1.2.5\n").unwrap();
        assert!(matches!(select(&root, "1.2.0", REQ, &f), Pick::Installed { .. }));
        install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        assert_eq!(high_water(&root).as_deref(), Some("1.3.0"));
        // The same version again with the mark at it: reused (the dir is there and checks out), nothing moves.
        let before = current(&root);
        install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        assert_eq!(current(&root), before);
        assert_eq!(
            pointer_id(&root, "previous"),
            None,
            "a reuse doesn't make the copy its own previous"
        );
        // A reused dir that no longer checks out isn't trusted.
        std::fs::write(
            root.join("versions")
                .join(&before)
                .join(ECHO_BUNDLE)
                .join(ECHO_EXE),
            "patched",
        )
        .unwrap();
        assert!(install(&root, &a, &e, "1.2.0", REQ, &f).is_err());
    }

    /// P3.3: prune keeps current, previous and the running copy, and clears staging leftovers.
    #[test]
    fn prune_never_removes_the_running_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = root_in(tmp.path());
        let f = Fake::default();
        let (a, e) = archive(tmp.path(), "1.3.0", "signed");
        install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        let first = current(&root);
        launched(&root, &first, Instant::now());
        std::fs::create_dir_all(root.join("versions/.staging-1.9.0-dead-1/qrtz-echo.app")).unwrap();
        for v in ["1.4.0", "1.5.0"] {
            let (b, e2) = archive(tmp.path(), v, "signed");
            install(&root, &b, &e2, "1.2.0", REQ, &f).unwrap();
        }
        let left: Vec<String> = std::fs::read_dir(root.join("versions"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left.len(), 3, "current, previous and the running 1.3.0: {left:?}");
        assert!(left.contains(&first), "{left:?}");
        assert!(!left.iter().any(|n| n.starts_with(".staging-")), "{left:?}");
    }

    /// P3.2: a downloaded echo that keeps crashing is set aside for the bundled one, and stays aside.
    #[test]
    fn a_crashing_copy_falls_back_to_the_bundled_echo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = root_in(tmp.path());
        let f = Fake::default();
        let (a, e) = archive(tmp.path(), "1.3.0", "signed");
        install(&root, &a, &e, "1.2.0", REQ, &f).unwrap();
        let id = current(&root);
        let t0 = Instant::now();
        // Starts spread out more than the window apart are healthy runs, not crashes.
        for i in 0..5u64 {
            assert!(!launched(
                &root,
                &id,
                t0 + (CRASH_WINDOW + Duration::from_secs(1)) * i as u32
            ));
        }
        let t1 = t0 + CRASH_WINDOW * 10;
        for i in 0..MAX_CRASHES {
            assert!(
                !launched(&root, &id, t1 + Duration::from_secs(u64::from(i))),
                "{i}"
            );
            assert!(matches!(select(&root, "1.2.0", REQ, &f), Pick::Installed { .. }));
        }
        assert!(launched(&root, &id, t1 + Duration::from_secs(10)));
        let p = select(&root, "1.2.0", REQ, &f);
        assert!(
            matches!(&p, Pick::Bundled { why: Some(w) } if w.contains("crashing")),
            "{p:?}"
        );
        assert!(
            bad_ids(&root).contains(&id),
            "persisted for the next launch of qrtz"
        );
    }

    /// E1: the running process must report ai.qrtz.echo, a version at least the floor, microphone-only.
    #[test]
    fn the_running_code_is_bound_to_version_and_entitlements() {
        let ok = RunningInfo {
            identifier: Some(ECHO_IDENTIFIER.into()),
            bundle_version: Some("1.4.0".into()),
            entitlements: vec!["com.apple.security.device.audio-input".into()],
        };
        assert!(check_running_info(&ok, "1.4.0").is_ok());
        assert!(check_running_info(&ok, "1.3.9").is_ok());
        let below = check_running_info(&ok, "1.4.1").unwrap_err();
        assert!(below.contains("below"), "{below}");
        let mut more = ok.clone();
        more.entitlements
            .push("com.apple.security.cs.disable-library-validation".into());
        assert!(check_running_info(&more, "1.0.0").is_err());
        let mut none = ok.clone();
        none.entitlements.clear();
        assert!(check_running_info(&none, "1.0.0").is_err());
        let mut other = ok.clone();
        other.identifier = Some("ai.qrtz.desktop".into());
        assert!(check_running_info(&other, "1.0.0").is_err());
        let mut nov = ok.clone();
        nov.bundle_version = None;
        assert!(check_running_info(&nov, "1.0.0").is_err());
        let mut junk = ok;
        junk.bundle_version = Some("latest".into());
        assert!(check_running_info(&junk, "1.0.0").is_err());
    }

    /// F1: the cdhash re-check names exactly the 20-byte cdhash of the running code.
    #[test]
    fn the_cdhash_requirement_names_the_running_codes_cdhash() {
        let unique: Vec<u8> = (0u8..20).map(|b| b * 13).collect();
        assert_eq!(
            cdhash_requirement(&unique).unwrap(),
            "cdhash H\"000d1a2734414e5b6875828f9ca9b6c3d0ddeaf7\""
        );
        assert!(cdhash_requirement(&[]).is_err());
        assert!(cdhash_requirement(&[0; 19]).is_err());
        assert!(cdhash_requirement(&[0; 32]).is_err());
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

    /// P3.4: the vectors mac-gate's e6 is tested on too (scripts/modules/entitlements.test.mjs).
    #[test]
    fn entitlement_vectors() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/echo-entitlements");
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let xml = std::fs::read_to_string(&p).unwrap();
            let ok = entitlements_are_mic_only(&xml).is_ok();
            assert_eq!(
                ok,
                name.starts_with("ok-"),
                "{name}: {:?}",
                granted_entitlements(&xml)
            );
            n += 1;
        }
        assert!(n >= 6, "{n} vectors");
    }
}
