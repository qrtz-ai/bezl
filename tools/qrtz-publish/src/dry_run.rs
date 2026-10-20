//! `dry-run`: the whole pipeline with test keys made at runtime (held in memory, never written), on a sample
//! extension, service and model: root, build, verify, pack, and a client sideload install of the pack.
use crate::keys::Keys;
use crate::{build, pack, verify, write_root, Spec, SpecPackage};
use qrtz_modules::index::PackageKind;
use qrtz_modules::install::{CodeChecks, InstallError, Installer};
use qrtz_modules::sideload::{Confirmation, Refusal, SideloadOrigin};
use std::path::Path;

pub const TEAM: &str = "DRYRUN0000";

/// Accepts every bundle: the dry run has no Developer ID signatures to check (said in its report).
pub struct NoAppleChecks;
impl CodeChecks for NoAppleChecks {
    fn check(&self, _: PackageKind, code: &Path, _: &str, _: &str) -> Result<(), String> {
        if code.exists() {
            Ok(())
        } else {
            Err(format!("{}: missing", code.display()))
        }
    }
}

fn write(p: &Path, b: &[u8], exec: bool) -> crate::Result<()> {
    let dir = p
        .parent()
        .ok_or_else(|| format!("{}: no parent folder", p.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(p, b).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    if exec {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    let _ = exec;
    Ok(())
}

fn json(p: &Path, v: serde_json::Value) -> crate::Result<()> {
    write(
        p,
        &serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?,
        false,
    )
}

/// A sample model (`model.sample`), service (`inference.sample`) and the nano extension that depends on both (ADR
/// 0174: echo is bundled in qrtz.app, never a published package).
/// `big` makes the model's weights 20 MiB, so the fetcher splits them into ranges.
/// The sample model's revision on its (fake) Hugging Face repo `qrtz/sample`.
pub const SAMPLE_REVISION: &str = "abc0000000000000000000000000000000000000";

/// What Hugging Face serves for [`sample`]'s model (ADR 0171: the index points there, never at the package host):
/// `(path under https://huggingface.co/, bytes)`, for a loopback server playing Hugging Face.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "dry-run sample: reads back files sample() just wrote"
)]
pub fn sample_hugging_face_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let model = dir.join("sample/model");
    let mut v = vec![];
    for rel in ["config.json", "weights/a.bin"] {
        let b = std::fs::read(model.join(rel)).expect("sample() wrote the model");
        v.push((format!("qrtz/sample/resolve/{SAMPLE_REVISION}/{rel}"), b));
    }
    v
}

pub fn sample(dir: &Path, big: bool) -> crate::Result<Spec> {
    let model = dir.join("sample/model");
    json(
        &model.join("qrtz.json"),
        serde_json::json!({"name": "model.sample", "version": "1.0.0", "kind": "model",
                           "model": {"task": "asr", "upstream": "qrtz/sample", "revision": SAMPLE_REVISION}}),
    )?;
    write(&model.join("config.json"), b"{\"sample\":true}", false)?;
    let n = if big { 20 * 1024 * 1024 } else { 300_000 };
    write(&model.join("weights/a.bin"), &vec![7u8; n], false)?;
    let svc = dir.join("sample/service");
    json(
        &svc.join("qrtz.json"),
        serde_json::json!({"name": "inference.sample", "version": "1.0.0", "kind": "service",
                           "service": {"bundle": "sample.xpc", "models": ["model.sample"]}}),
    )?;
    write(
        &svc.join("sample.xpc/Contents/MacOS/sample"),
        b"#!/bin/sh\n",
        true,
    )?;
    write(&svc.join("sample.xpc/Contents/Info.plist"), b"<plist/>", false)?;
    let nano = dir.join("sample/nano");
    json(
        &nano.join("qrtz.json"),
        serde_json::json!({"name": "nano", "version": "0.3.0", "kind": "extension",
            "dependencies": {"inference.sample": "^1.0.0", "model.sample": "^1.0.0"},
            "extension": {"entry": "bin/qrtz-nano", "protocol": 1, "minApp": "0.3.0",
                          "channels": ["recording.*"], "views": ["note"], "permissions": ["microphone"]}}),
    )?;
    write(&nano.join("bin/qrtz-nano"), b"#!/bin/sh\necho dry-run\n", true)?;
    Ok(Spec {
        team_id: TEAM.into(),
        packages: [model, svc, nano]
            .into_iter()
            .map(SpecPackage::fake_notarized)
            .collect(),
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    })
}

/// Run it all in `dir`; the report lists what was made and checked. No key is printed or written.
pub fn run(dir: &Path, big: bool) -> crate::Result<Vec<String>> {
    let now = crate::now_ms();
    let keys = Keys::ephemeral();
    let out = dir.join("out");
    let mut report = vec![];
    let (n, pin) = write_root(&out, &keys, None, 1, now)?;
    report.push(format!("root {n} written (1-of-2 root keys); pin sha256 {pin}"));
    let spec = sample(dir, big)?;
    let b = build(&out, &spec, &keys, now)?;
    report.push(format!(
        "index {} ({}), timestamp {}; added {}",
        b.index_version,
        &b.index_sha256[..12],
        b.timestamp_version,
        b.added.join(", ")
    ));
    report.push(verify(&out, &pin, now)?);
    let pkg = dir.join("nano-0.3.0.qrtzpkg");
    let set = pack(&out, "nano", "0.3.0", &pkg)?;
    report.push(format!(
        "packed {} ({} bytes): {}",
        pkg.file_name().unwrap_or_default().to_string_lossy(),
        std::fs::metadata(&pkg).map(|m| m.len()).unwrap_or(0),
        set.join(", ")
    ));
    // The client side: a sideload into an empty root, prompt first, then the confirmed install.
    let client = Installer::new(dir.join("client"), Some(pin), Box::new(NoAppleChecks));
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| e.to_string())?;
    let prompt = match rt.block_on(client.sideload(&pkg, SideloadOrigin::UserPickedFile, None, now)) {
        Err(InstallError::Refused(Refusal::NeedsConfirmation(p))) => p,
        other => return Err(format!("expected the confirmation prompt, got {other:?}")),
    };
    let yes = Confirmation {
        package_sha256: prompt.package_sha256.clone(),
        revocation_issued_ms: prompt.revocation_issued_ms,
    };
    let done = rt
        .block_on(client.sideload(&pkg, SideloadOrigin::UserPickedFile, Some(&yes), now))
        .map_err(|e| e.to_string())?;
    report.push(format!(
        "client sideload installed {} packages into {} (stale revocation: {}); apple signature checks skipped (no developer id in a dry run)",
        done.changes.len(),
        client.root().join("packages").display(),
        done.stale_revocation
    ));
    Ok(report)
}
