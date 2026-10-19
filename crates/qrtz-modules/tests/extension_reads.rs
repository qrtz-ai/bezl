//! ADR 0212: every runnable unit declares the models it reads (`extension.reads` for the extension's own process,
//! `service.models` for a service), and `resolve::extension_grants` alone derives grants from those declarations:
//! no orphan model dependency, no read of anything that isn't a model dependency. No allowlist names a package.
use qrtz_modules::qrtz_json::PackageManifest;
use qrtz_modules::resolve::{extension_grants, Grant, ResolveError};
use std::collections::BTreeMap;

fn ext(name: &str, deps: serde_json::Value, reads: serde_json::Value) -> Result<PackageManifest, String> {
    let v = serde_json::json!({"name": name, "version": "1.0.0", "kind": "extension", "dependencies": deps,
        "extension": {"entry": "bin/h", "protocol": 1, "minApp": "0.3.0", "reads": reads}});
    PackageManifest::parse(&serde_json::to_vec(&v).unwrap()).map_err(|e| e.to_string())
}

fn deps(d: &[&str]) -> BTreeMap<String, String> {
    d.iter().map(|n| ((*n).to_owned(), "1.0.0".to_owned())).collect()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

fn none(_: &str, _: &str) -> Option<(String, Vec<String>)> {
    None
}

#[test]
fn any_extension_may_declare_reads_of_its_own_model_dependencies() {
    ext(
        "ctrl",
        serde_json::json!({"model.laya": "^1.0.0"}),
        serde_json::json!(["model.laya"]),
    )
    .unwrap();
    ext(
        "nano",
        serde_json::json!({"model.localvqe": "^1.0.0", "model.nemotron": "^1.0.0", "model.nemotron-diarizer": "^1.0.0"}),
        serde_json::json!(["model.localvqe", "model.nemotron", "model.nemotron-diarizer"]),
    )
    .unwrap();
}

#[test]
fn a_read_of_a_non_dependency_a_non_model_or_a_repeat_is_refused_by_the_manifest() {
    for (name, deps, reads) in [
        // nano reading laya without depending on it.
        (
            "nano",
            serde_json::json!({"model.nemotron": "^1.0.0"}),
            serde_json::json!(["model.laya"]),
        ),
        // ctrl must depend on what it reads.
        ("ctrl", serde_json::json!({}), serde_json::json!(["model.laya"])),
        // A service isn't a model.
        (
            "nano",
            serde_json::json!({"inference.voice": "^1.0.0"}),
            serde_json::json!(["inference.voice"]),
        ),
        // Twice.
        (
            "ctrl",
            serde_json::json!({"model.laya": "^1.0.0"}),
            serde_json::json!(["model.laya", "model.laya"]),
        ),
    ] {
        let e = ext(name, deps, reads).unwrap_err();
        assert!(e.contains("extension.reads"), "{name}: {e}");
    }
}

#[test]
fn grants_are_exactly_the_declared_reads() {
    let g = extension_grants(
        "nano",
        "0.2.2",
        &deps(&["model.localvqe", "model.nemotron", "model.nemotron-diarizer"]),
        &s(&["model.localvqe", "model.nemotron", "model.nemotron-diarizer"]),
        &none,
    )
    .unwrap();
    let models: Vec<&str> = g.iter().map(|g| g.model.as_str()).collect();
    assert_eq!(
        models,
        ["model.localvqe", "model.nemotron", "model.nemotron-diarizer"]
    );
    assert!(g
        .iter()
        .all(|g| g.own_process && g.unit == "nano" && g.model_version == "1.0.0"));
}

#[test]
fn an_orphan_model_dependency_is_refused() {
    // nano 0.2.1 as published in index 2: three model dependencies, nothing that reads them.
    let e = extension_grants(
        "nano",
        "0.2.1",
        &deps(&["model.localvqe", "model.nemotron", "model.nemotron-diarizer"]),
        &[],
        &none,
    )
    .unwrap_err();
    assert!(
        matches!(e, ResolveError::NoReader { ref model, .. } if model == "model.localvqe"),
        "{e}"
    );
    // Reading two of three still leaves an orphan.
    let e = extension_grants(
        "nano",
        "0.2.2",
        &deps(&["model.localvqe", "model.nemotron", "model.nemotron-diarizer"]),
        &s(&["model.localvqe", "model.nemotron"]),
        &none,
    )
    .unwrap_err();
    assert!(
        matches!(e, ResolveError::NoReader { ref model, .. } if model == "model.nemotron-diarizer"),
        "{e}"
    );
}

#[test]
fn an_undeclared_read_is_refused_and_a_service_gets_only_this_extensions_models() {
    // The process reads a model it doesn't depend on (nano → laya).
    let e = extension_grants(
        "nano",
        "0.2.2",
        &deps(&["model.nemotron"]),
        &s(&["model.nemotron", "model.laya"]),
        &none,
    )
    .unwrap_err();
    assert!(
        matches!(e, ResolveError::ReadNotDependency { ref unit, ref model, .. } if unit == "nano" && model == "model.laya"),
        "{e}"
    );
    // A service that declares a model its extension doesn't depend on is not granted it (for this extension).
    let svc = |n: &str, _: &str| {
        (n == "inference.voice").then(|| ("1.0.0".to_owned(), s(&["model.nemotron", "model.laya"])))
    };
    let g = extension_grants(
        "nano",
        "0.2.2",
        &deps(&["inference.voice", "model.nemotron"]),
        &[],
        &svc,
    )
    .unwrap();
    assert!(
        g.iter().all(|g| g.model == "model.nemotron" && !g.own_process),
        "{g:?}"
    );
}

#[test]
fn a_service_grant_belongs_to_the_service_not_the_process() {
    let svc =
        |n: &str, _: &str| (n == "inference.voice").then(|| ("1.4.0".to_owned(), s(&["model.nemotron"])));
    let g = extension_grants(
        "nano",
        "0.3.0",
        &deps(&["inference.voice", "model.nemotron"]),
        &[],
        &svc,
    )
    .unwrap();
    assert_eq!(
        g,
        [Grant {
            extension: "nano".into(),
            unit: "inference.voice".into(),
            unit_version: "1.4.0".into(),
            own_process: false,
            model: "model.nemotron".into(),
            model_version: "1.0.0".into(),
        }]
    );
}

/// Every package manifest in the repo passes the same check the installer and the publisher run.
#[test]
fn every_repo_module_manifest_passes_the_read_check() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let mut n = 0;
    for e in std::fs::read_dir(&root).unwrap().flatten() {
        let p = e.path().join("qrtz.json");
        if !p.is_file() {
            continue;
        }
        let m = PackageManifest::parse(&std::fs::read(&p).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let reads = m.extension.as_ref().map(|x| x.reads.clone()).unwrap_or_default();
        extension_grants(&m.name, &m.version, &m.dependencies, &reads, &none)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        n += 1;
    }
    assert!(n >= 4, "found only {n} modules/*/qrtz.json");
}

/// The bug this ADR fixes: the live index 2 (snapshot in spec/fixtures/ctrl-live-index) carries nano 0.2.0 and
/// 0.2.1 with three model dependencies and no reads. The installer refuses them, and so would the publisher.
#[test]
fn the_live_index_2_nano_is_refused_at_install_and_would_be_at_publish() {
    use qrtz_modules::index::Index;
    use qrtz_modules::qrtz_lock::{QrtzLock, Workspace};
    use qrtz_modules::resolve::{check_index_reads, resolve, Update};
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/fixtures/ctrl-live-index/index.json");
    let ix = Index::parse(&std::fs::read(p).unwrap()).unwrap();
    let mut ws = Workspace::default();
    ws.dependencies.insert("nano".into(), "^0.2.0".into());
    let e = resolve(&ix, &ws, &QrtzLock::default(), &Update::None).unwrap_err();
    assert!(
        matches!(e, ResolveError::NoReader { ref extension, .. } if extension == "nano"),
        "{e}"
    );
    let e = check_index_reads(&ix.packages, &ix.revoked).unwrap_err();
    assert!(
        matches!(e, ResolveError::NoReader { ref extension, .. } if extension == "nano"),
        "{e}"
    );
    // ADR 0215: with both nano versions revoked (modules/revoked.json) every resolvable entry passes.
    let revoked: Vec<qrtz_modules::sideload::Revoked> = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules/revoked.json"))
            .unwrap(),
    )
    .unwrap();
    check_index_reads(&ix.packages, &revoked).unwrap();
    // ctrl's declared read resolves as before.
    let mut ws = Workspace::default();
    ws.dependencies.insert("ctrl".into(), "^0.2.0".into());
    resolve(&ix, &ws, &QrtzLock::default(), &Update::None).unwrap();
}
