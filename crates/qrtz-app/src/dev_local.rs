//! `scripts/dev/dev-local.sh` (docs/dev/dev-local.md): a debug build runs nano, snap, ctrl and echo from this
//! checkout's own `.build/debug` products instead of the installed packages.
//!
//! Gates (Bozniak's dev-local review), all required:
//! - compiled in only with `debug_assertions` AND the non-default `dev-local` feature ([`ENABLED`]); a release
//!   build has none of this module's code or strings (`scripts/release/verify-bundle.sh` checks the binary);
//! - `QRTZ_USER_DATA` set, absolute, and neither the real data dir nor inside it; nothing is copied from it;
//! - each `QRTZ_<ID>_BIN` absolute, canonical (no symlink anywhere, so none in the final component), a regular
//!   executable file owned by this user, not group- or world-writable, under the checkout (`QRTZ_DEV_REPO`) or
//!   `CARGO_TARGET_DIR`;
//! - the checkout's `modules/<id>/module.json` parses through the shared checker and declares nothing beyond this
//!   build's manifest for that id (one generic rule for every module id);
//! - all four modules by default (`--without <id>` leaves one out); ctrl still goes through its own consent and
//!   enable prompts in the isolated data dir: an override marks a module installed, never enabled (ADR 0216).
//!
//! An override skips only the signature and lock-hash check: the module then counts as installed (no qrtz.lock
//! entry) and runs from the given path under `LaunchCheck::Dev`. Any failure refuses the whole start.
#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
use serde_json::Value;
use std::collections::BTreeMap;
#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Whether this build can honour dev-local overrides at all: decided at compile time.
pub const ENABLED: bool = cfg!(all(debug_assertions, feature = "dev-local"));

/// The modules dev-local can run from the checkout.
pub const IDS: [&str; 4] = ["nano", "snap", "ctrl", "echo"];

/// One module run from the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub id: String,
    pub bin: PathBuf,
    pub version: String,
}

#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
/// Where the inputs come from (the environment in the app; fixed values in tests).
pub struct Inputs<'a> {
    pub get: &'a dyn Fn(&str) -> Option<String>,
    /// The real data dir, resolved the way `paths::data_dir` does without `QRTZ_USER_DATA`.
    pub real_data: Option<PathBuf>,
    /// This user's uid.
    pub uid: u32,
}

#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
/// The overrides to honour, or the reason to refuse the start. `enabled` is [`ENABLED`] in the app: when false
/// (any release build) the environment is ignored entirely.
pub fn resolve(enabled: bool, i: &Inputs) -> Result<Vec<Override>, String> {
    if !enabled {
        return Ok(vec![]);
    }
    let var = |k: &str| (i.get)(k).filter(|v| !v.is_empty());
    let wanted: Vec<(&str, String)> = IDS
        .iter()
        .filter_map(|id| var(&format!("QRTZ_{}_BIN", id.to_uppercase())).map(|p| (*id, p)))
        .collect();
    if wanted.is_empty() {
        return Ok(vec![]);
    }
    check_user_data(var("QRTZ_USER_DATA"), i.real_data.as_deref())?;
    let repo = var("QRTZ_DEV_REPO")
        .ok_or("dev-local: QRTZ_DEV_REPO (the checkout) is not set")
        .and_then(|r| canonical_dir(&r).ok_or("dev-local: QRTZ_DEV_REPO is not an existing absolute dir"))?;
    let mut roots = vec![repo.clone()];
    if let Some(t) = var("CARGO_TARGET_DIR").and_then(|t| canonical_dir(&t)) {
        roots.push(t);
    }
    wanted
        .into_iter()
        .map(|(id, p)| {
            let bin = check_bin(id, Path::new(&p), &roots, i.uid)?;
            let text = std::fs::read(repo.join("modules").join(id).join("module.json"))
                .map_err(|e| format!("dev-local: {id}: modules/{id}/module.json: {e}"))?;
            let version = check_manifest(id, &text, &bundled_manifest(id)?)?;
            Ok(Override {
                id: id.to_owned(),
                bin,
                version,
            })
        })
        .collect()
}

#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
fn canonical_dir(p: &str) -> Option<PathBuf> {
    let p = Path::new(p);
    if !p.is_absolute() {
        return None;
    }
    std::fs::canonicalize(p).ok().filter(|c| c.is_dir())
}

#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
/// `QRTZ_USER_DATA` is set, absolute, and neither the real data dir nor inside it (compared canonically when it
/// exists, so a link to the real dir is refused too).
pub fn check_user_data(user_data: Option<String>, real: Option<&Path>) -> Result<PathBuf, String> {
    let d = user_data.ok_or("dev-local: QRTZ_USER_DATA must be set to an isolated data dir")?;
    let d = PathBuf::from(d);
    if !d.is_absolute() {
        return Err(format!(
            "dev-local: QRTZ_USER_DATA {} is not absolute",
            d.display()
        ));
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let real = real.ok_or("dev-local: can't tell where the real data dir is, so refusing")?;
    let (dc, rc) = (canon(&d), canon(real));
    if dc.starts_with(&rc) || d.starts_with(real) {
        return Err(format!(
            "dev-local: QRTZ_USER_DATA {} is the real data dir or inside it ({}): refused",
            d.display(),
            real.display()
        ));
    }
    Ok(dc)
}

#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
/// A module binary dev-local may run (see the module doc).
pub fn check_bin(id: &str, p: &Path, roots: &[PathBuf], uid: u32) -> Result<PathBuf, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let refuse = |why: &str| Err(format!("dev-local: {id}: {} {why}: refused", p.display()));
    if !p.is_absolute() {
        return refuse("is not absolute");
    }
    let Ok(lm) = std::fs::symlink_metadata(p) else {
        return refuse("doesn't exist");
    };
    if lm.file_type().is_symlink() {
        return refuse("is a symlink");
    }
    if std::fs::canonicalize(p).ok().as_deref() != Some(p) {
        return refuse("is not canonical (a symlink or `..` on the way)");
    }
    if !lm.file_type().is_file() {
        return refuse("is not a regular file");
    }
    let mode = lm.permissions().mode();
    if mode & 0o111 == 0 {
        return refuse("is not executable");
    }
    if mode & 0o022 != 0 {
        return refuse("is group- or world-writable");
    }
    if lm.uid() != uid {
        return refuse("is not owned by you");
    }
    if !roots.iter().any(|r| p.starts_with(r)) {
        return refuse("is outside the checkout and CARGO_TARGET_DIR");
    }
    Ok(p.to_path_buf())
}

#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
fn bundled_manifest(id: &str) -> Result<Value, String> {
    qrtz_modules::bundled::manifests()
        .iter()
        .find(|(m, _)| *m == id)
        .and_then(|(_, t)| serde_json::from_str(t).ok())
        .ok_or_else(|| format!("dev-local: {id}: this build has no manifest for it"))
}

#[cfg(any(test, all(debug_assertions, feature = "dev-local")))]
/// The checkout's module.json for `id`: valid by the shared checker (ADR 0212 rules included), the same id, and
/// declaring nothing this build's manifest for `id` doesn't. Returns its version.
pub fn check_manifest(id: &str, text: &[u8], built: &Value) -> Result<String, String> {
    let m = qrtz_modules::manifest::ModuleManifest::parse(text)
        .map_err(|e| format!("dev-local: {id}: module.json is invalid: {e}"))?;
    if m.id != id {
        return Err(format!("dev-local: {id}: module.json is for {:?}", m.id));
    }
    let raw: Value = serde_json::from_slice(text).map_err(|e| e.to_string())?;
    let list = |v: &Value, path: &[&str]| -> Vec<Value> {
        path.iter()
            .try_fold(v, |v, k| v.get(*k))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    for path in [
        &["capabilities"][..],
        &["permissions"],
        &["entitlements"],
        &["views"],
        &["provides"],
        &["dependencies"],
        &["launch", "channels"],
    ] {
        let allowed = list(built, path);
        if let Some(extra) = list(&raw, path).into_iter().find(|x| !allowed.contains(x)) {
            return Err(format!(
                "dev-local: {id}: module.json declares {} {extra} beyond this build's manifest: refused",
                path.join(".")
            ));
        }
    }
    for k in ["code"] {
        if raw.get(k) != built.get(k) {
            return Err(format!("dev-local: {id}: module.json changes `{k}`: refused"));
        }
    }
    Ok(m.version)
}

static ACTIVE: OnceLock<Vec<Override>> = OnceLock::new();

/// Resolve from the environment once at start (logging `dev-local: <id> from <path>` per module) and remember the
/// result. `Err` means the app must not start.
#[cfg(all(debug_assertions, feature = "dev-local"))]
pub fn activate(real_data: Option<PathBuf>) -> Result<&'static [Override], String> {
    let get = |k: &str| std::env::var(k).ok();
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let o = resolve(
        ENABLED,
        &Inputs {
            get: &get,
            real_data,
            uid,
        },
    )?;
    for x in &o {
        tracing::warn!("dev-local: {} from {}", x.id, x.bin.display());
        eprintln!("dev-local: {} from {}", x.id, x.bin.display());
    }
    Ok(ACTIVE.get_or_init(|| o).as_slice())
}

/// Without dev-local compiled in there is nothing to activate (and none of its strings are in the binary).
#[cfg(not(all(debug_assertions, feature = "dev-local")))]
pub fn activate(_real_data: Option<PathBuf>) -> Result<&'static [Override], String> {
    Ok(&[])
}

/// The active overrides (empty unless [`activate`] honoured some).
pub fn active() -> &'static [Override] {
    ACTIVE.get().map_or(&[], Vec::as_slice)
}

/// The dev-local overrides as a helper layout's `programs`.
pub fn programs() -> BTreeMap<String, PathBuf> {
    active().iter().map(|o| (o.id.clone(), o.bin.clone())).collect()
}

/// The lock gate: an active override counts as installed, at its checkout version.
pub fn merge_installed(im: &mut crate::installed::InstalledModules) {
    merge_into(active(), im);
}

fn merge_into(o: &[Override], im: &mut crate::installed::InstalledModules) {
    for x in o {
        im.patch["modules"][&x.id] = serde_json::json!({"installed": true});
        if !qrtz_modules::is_bundled_extension(&x.id) {
            im.versions.insert(x.id.clone(), x.version.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn uid() -> u32 {
        unsafe { libc::getuid() }
    }

    /// A checkout with a built `qrtz-<id>` and its module.json, plus a real and a dev data dir.
    struct Fixture {
        _t: tempfile::TempDir,
        repo: PathBuf,
        real: PathBuf,
        dev: PathBuf,
    }

    fn fixture() -> Fixture {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        let repo = root.join("repo");
        let bin = repo.join("native/qrtz-helper/.build/debug");
        std::fs::create_dir_all(&bin).unwrap();
        for (id, text) in qrtz_modules::bundled::manifests() {
            std::fs::create_dir_all(repo.join("modules").join(id)).unwrap();
            std::fs::write(repo.join("modules").join(id).join("module.json"), text).unwrap();
            let b = bin.join(format!("qrtz-{id}"));
            std::fs::write(&b, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let real = root.join("real/ai.qrtz.desktop");
        let dev = root.join("dev/ai.qrtz.desktop-dev");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&dev).unwrap();
        Fixture {
            _t: t,
            repo,
            real,
            dev,
        }
    }

    fn run(f: &Fixture, enabled: bool, extra: &[(&str, String)]) -> Result<Vec<Override>, String> {
        let bin = f.repo.join("native/qrtz-helper/.build/debug");
        let mut env: BTreeMap<String, String> = IDS
            .iter()
            .map(|id| {
                (
                    format!("QRTZ_{}_BIN", id.to_uppercase()),
                    bin.join(format!("qrtz-{id}")).display().to_string(),
                )
            })
            .collect();
        env.insert("QRTZ_USER_DATA".into(), f.dev.display().to_string());
        env.insert("QRTZ_DEV_REPO".into(), f.repo.display().to_string());
        for (k, v) in extra {
            env.insert((*k).into(), v.clone());
        }
        let get = move |k: &str| env.get(k).cloned();
        resolve(
            enabled,
            &Inputs {
                get: &get,
                real_data: Some(f.real.clone()),
                uid: uid(),
            },
        )
    }

    fn ids(o: &[Override]) -> Vec<&str> {
        o.iter().map(|x| x.id.as_str()).collect()
    }

    #[test]
    fn only_a_debug_build_with_the_feature_is_enabled() {
        assert_eq!(ENABLED, cfg!(all(debug_assertions, feature = "dev-local")));
        if !cfg!(debug_assertions) {
            assert!(!ENABLED);
        }
    }

    /// A release build (enabled = false) ignores the whole environment, even a fully valid one.
    #[test]
    fn release_ignores_the_env() {
        let f = fixture();
        assert_eq!(run(&f, false, &[]).unwrap(), vec![]);
        assert!(!run(&f, true, &[]).unwrap().is_empty());
    }

    #[test]
    fn a_missing_or_real_user_data_refuses() {
        let f = fixture();
        let e = run(&f, true, &[("QRTZ_USER_DATA", String::new())]).unwrap_err();
        assert!(e.contains("must be set"), "{e}");
        let e = run(&f, true, &[("QRTZ_USER_DATA", "rel/dir".into())]).unwrap_err();
        assert!(e.contains("not absolute"), "{e}");
        for d in [
            f.real.clone(),
            f.real.join("sub"),
            f.real.join("../ai.qrtz.desktop"),
        ] {
            let e = run(&f, true, &[("QRTZ_USER_DATA", d.display().to_string())]).unwrap_err();
            assert!(e.contains("real data dir"), "{d:?}: {e}");
        }
        let link = f.dev.parent().unwrap().join("link");
        std::os::unix::fs::symlink(&f.real, &link).unwrap();
        let e = run(&f, true, &[("QRTZ_USER_DATA", link.display().to_string())]).unwrap_err();
        assert!(e.contains("real data dir"), "{e}");
    }

    #[test]
    fn a_bad_perms_or_symlinked_bin_refuses() {
        let f = fixture();
        let dir = f.repo.join("native/qrtz-helper/.build/debug");
        let nano = dir.join("qrtz-nano");
        std::fs::set_permissions(&nano, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(run(&f, true, &[]).unwrap_err().contains("writable"));
        std::fs::set_permissions(&nano, std::fs::Permissions::from_mode(0o757)).unwrap();
        assert!(run(&f, true, &[]).unwrap_err().contains("writable"));
        std::fs::set_permissions(&nano, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(run(&f, true, &[]).unwrap_err().contains("not executable"));
        std::fs::set_permissions(&nano, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.join("nano-link");
        std::os::unix::fs::symlink(&nano, &link).unwrap();
        let e = run(&f, true, &[("QRTZ_NANO_BIN", link.display().to_string())]).unwrap_err();
        assert!(e.contains("symlink"), "{e}");
        let ldir = f.repo.join("linkdir");
        std::os::unix::fs::symlink(&dir, &ldir).unwrap();
        let e = run(
            &f,
            true,
            &[("QRTZ_NANO_BIN", ldir.join("qrtz-nano").display().to_string())],
        )
        .unwrap_err();
        assert!(e.contains("not canonical"), "{e}");
        let e = run(&f, true, &[("QRTZ_NANO_BIN", "rel/qrtz-nano".into())]).unwrap_err();
        assert!(e.contains("not absolute"), "{e}");
        let outside = f.dev.join("qrtz-nano");
        std::fs::write(&outside, b"x").unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = run(&f, true, &[("QRTZ_NANO_BIN", outside.display().to_string())]).unwrap_err();
        assert!(e.contains("outside the checkout"), "{e}");
        let e = run(&f, true, &[("QRTZ_NANO_BIN", dir.display().to_string())]).unwrap_err();
        assert!(e.contains("not a regular file"), "{e}");
    }

    #[test]
    fn an_over_declaring_module_json_is_refused() {
        for id in IDS {
            let (_, text) = qrtz_modules::bundled::manifests()
                .into_iter()
                .find(|(m, _)| *m == id)
                .unwrap();
            let built: Value = serde_json::from_str(text).unwrap();
            assert!(check_manifest(id, text.as_bytes(), &built).is_ok(), "{id}");
            let mut more = built.clone();
            more["capabilities"]
                .as_array_mut()
                .unwrap()
                .push("ctrl.do".into());
            let e = check_manifest(id, more.to_string().as_bytes(), &built).unwrap_err();
            assert!(e.contains("beyond this build's manifest"), "{id}: {e}");
            let mut more = built.clone();
            more["entitlements"]
                .as_array_mut()
                .unwrap()
                .push("com.apple.security.get-task-allow".into());
            assert!(
                check_manifest(id, more.to_string().as_bytes(), &built).is_err(),
                "{id}"
            );
            let mut more = built.clone();
            more["launch"]["channels"]
                .as_array_mut()
                .unwrap()
                .push("calendar.*".into());
            assert!(
                check_manifest(id, more.to_string().as_bytes(), &built).is_err(),
                "{id}"
            );
            let other = if id == "nano" { "snap" } else { "nano" };
            assert!(check_manifest(other, text.as_bytes(), &built).is_err(), "{id}");
            assert!(check_manifest(id, b"{}", &built).is_err());
        }
        let f = fixture();
        let p = f.repo.join("modules/snap/module.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        v["permissions"].as_array_mut().unwrap().push("microphone".into());
        std::fs::write(&p, v.to_string()).unwrap();
        assert!(run(&f, true, &[]).unwrap_err().contains("snap"));
    }

    /// All four by default; an override marks a module installed and never switches it on (ctrl's consent and
    /// enable prompts still decide, ADR 0216). Leaving one's variable unset (`--without <id>`) leaves it out.
    #[test]
    fn ctrl_is_installed_but_never_enabled() {
        let f = fixture();
        let o = run(&f, true, &[]).unwrap();
        assert_eq!(ids(&o), ["nano", "snap", "ctrl", "echo"]);
        let o = run(&f, true, &[("QRTZ_CTRL_BIN", String::new())]).unwrap();
        assert_eq!(ids(&o), ["nano", "snap", "echo"]);
        let mut im = crate::installed::installed_modules(&Default::default());
        merge_into(&run(&f, true, &[]).unwrap(), &mut im);
        assert_eq!(
            im.patch["modules"]["ctrl"],
            serde_json::json!({"installed": true})
        );
        assert!(im.patch["modules"]["ctrl"].get("enabled").is_none());
    }
}
