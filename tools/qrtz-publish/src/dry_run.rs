//! `dry-run`: the whole pipeline with test keys made at runtime (held in memory, never written), on a sample
//! module and model set: root, build, timestamp, pack, verify, and a client sideload install of the pack.
use crate::keys::Keys;
use crate::{build, pack, verify, write_root, Spec, SpecPackage};
use qrtz_modules::install::{CodeChecks, Installer};
use qrtz_modules::lock::content_digest;
use qrtz_modules::manifest::{designated_requirement, ModuleManifest};
use qrtz_modules::sideload::{Confirmation, Refusal, SideloadOrigin};
use std::path::Path;

pub const TEAM: &str = "DRYRUN0000";

/// Accepts every bundle: the dry run has no Developer ID signatures to check (said in its report).
pub struct NoAppleChecks;
impl CodeChecks for NoAppleChecks {
    fn check_module(&self, _: &Path, _: &ModuleManifest) -> Result<(), String> {
        Ok(())
    }
    fn check_service(&self, _: &Path, _: &str, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A sample model set and an echo module that pins it. `big` adds a file just over one chunk.
pub fn sample(dir: &Path, big: bool) -> crate::Result<Spec> {
    let model = dir.join("sample/model");
    std::fs::create_dir_all(model.join("weights")).map_err(|e| e.to_string())?;
    std::fs::write(model.join("config.json"), b"{\"sample\":true}").map_err(|e| e.to_string())?;
    std::fs::write(model.join("weights/a.bin"), vec![7u8; 300_000]).map_err(|e| e.to_string())?;
    if big {
        let f = std::fs::File::create(model.join("weights/big.bin")).map_err(|e| e.to_string())?;
        f.set_len(qrtz_modules::index::CHUNK_SIZE + 4096)
            .map_err(|e| e.to_string())?;
    }
    let mut files = vec![];
    for rel in ["config.json", "weights/a.bin", "weights/big.bin"] {
        let p = model.join(rel);
        if let Ok((n, h)) = qrtz_modules::fsutil::sha256_file(&p) {
            files.push((rel.to_owned(), n, h));
        }
    }
    let digest = content_digest(files.iter().map(|(p, n, h)| (p.as_str(), *n, h.as_str())));
    let echo = dir.join("sample/echo");
    let exe = echo.join("qrtz-echo.app/Contents/MacOS");
    std::fs::create_dir_all(&exe).map_err(|e| e.to_string())?;
    std::fs::write(exe.join("qrtz-echo"), b"#!/bin/sh\necho dry-run\n").map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(exe.join("qrtz-echo"), std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    std::fs::write(echo.join("qrtz-echo.app/Contents/Info.plist"), b"<plist/>").map_err(|e| e.to_string())?;
    let b = "ai.qrtz.echo";
    let m = serde_json::json!({
        "schema": 1, "id": "echo", "name": "echo", "version": "0.3.0", "protocol": 1, "minApp": "0.2.0",
        "code": {"bundleId": b, "teamId": TEAM, "designatedRequirement": designated_requirement(b, TEAM)},
        "launch": {"kind": "launch-services", "entry": "qrtz-echo.app", "channels": ["dictation.*"]},
        "views": ["talk"], "capabilities": ["talk.read"], "permissions": ["microphone"],
        "entitlements": ["com.apple.security.device.audio-input"],
        "settings": {"namespace": "echo", "legacySections": ["talk"]},
        "dependencies": [{"id": "model:sample", "kind": "data", "version": "1.0.0", "sha256": digest, "teamId": TEAM}],
    });
    std::fs::write(
        echo.join("module.json"),
        serde_json::to_vec_pretty(&m).expect("json"),
    )
    .map_err(|e| e.to_string())?;
    Ok(Spec {
        team_id: TEAM.into(),
        packages: vec![
            SpecPackage {
                id: "model:sample".into(),
                version: "1.0.0".into(),
                dir: model,
                min_os: None,
            },
            SpecPackage {
                id: "echo".into(),
                version: "0.3.0".into(),
                dir: echo,
                min_os: Some("14.2".into()),
            },
        ],
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
    let pkg = dir.join("echo-0.3.0.qrtzpkg");
    pack(&out, "echo", "0.3.0", &pkg)?;
    report.push(format!(
        "packed {} ({} bytes)",
        pkg.file_name().unwrap_or_default().to_string_lossy(),
        std::fs::metadata(&pkg).map(|m| m.len()).unwrap_or(0)
    ));
    // The client side: a sideload into an empty modules dir, prompt first, then the confirmed install.
    let client = Installer::new(dir.join("client"), Some(pin), Box::new(NoAppleChecks));
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| e.to_string())?;
    let prompt = match rt.block_on(client.sideload(&pkg, SideloadOrigin::UserPickedFile, None, now)) {
        Err(qrtz_modules::install::InstallError::Refused(Refusal::NeedsConfirmation(p))) => p,
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
        "client sideload installed {}@{} (stale revocation: {}); apple signature checks skipped (no developer id in a dry run)",
        done.module, done.version, done.stale_revocation
    ));
    Ok(report)
}
