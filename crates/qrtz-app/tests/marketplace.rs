//! ADR 0161 on loopback: the marketplace's catalog is the stored signed index (read with no network, and empty
//! before the first check), each extension's card carries its dependency tree, install size and permissions, and
//! `packages:apply` reports its download as `packages:progress` (at most once per percent, ending at the total).
#![cfg(feature = "runtime")]
use qrtz_app::module_hosting::{HttpTransport, Packages};
use qrtz_app::packages_ipc::PackagesIpc;
use qrtz_app::ports::PackagesPort;
use qrtz_app::service::Events;
use qrtz_modules::install::Installer;
use qrtz_publish::dry_run::{sample, sample_hugging_face_files, NoAppleChecks};
use qrtz_publish::keys::Keys;
use qrtz_publish::{build, write_root};
use qrtz_speech::testkit::Server;
use serde_json::{json, Value};
use std::path::Path;

fn serve(dir: &Path, base: &Path, s: &Server) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            serve(&p, base, s);
        } else {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            s.put(&rel, std::fs::read(&p).unwrap());
        }
    }
}

#[tokio::test]
async fn the_catalog_comes_from_the_signed_index_with_sizes_dependencies_and_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let out = dir.join("out");
    let keys = Keys::ephemeral();
    let now = qrtz_publish::now_ms();
    let (_, pin) = write_root(&out, &keys, None, 1, now).unwrap();
    build(&out, &sample(&dir, true).unwrap(), &keys, now).unwrap();
    let server = Server::start().await;
    serve(&out, &out, &server);
    // ADR 0171: the model's files come from (a loopback) Hugging Face, not the package host.
    let hf = Server::start().await;
    for (p, b) in sample_hugging_face_files(&dir) {
        hf.put(&p, b);
    }
    let packages = Packages::with(
        Installer::new(dir.join("ui"), Some(pin), Box::new(NoAppleChecks)),
        HttpTransport::new(
            std::slice::from_ref(&server.base),
            &[("https://huggingface.co".into(), hf.base.clone())],
        ),
    );
    let events = Events::new();
    let mut rx = events.subscribe();
    let ipc = PackagesIpc::new(packages).with_events(events);
    let contract = qrtz_ipc::contract();
    let hits = || -> usize { server.faults.lock().unwrap().hits.values().sum() };

    // Before the first check: nothing to browse, and listing never reaches the host.
    let s = ipc.call("packages:list", Value::Null).await.unwrap();
    assert_eq!(s["available"], json!([]));
    assert_eq!(hits(), 0);

    ipc.call("packages:refresh", Value::Null).await.unwrap();
    let before = hits();
    let s = ipc.call("packages:list", Value::Null).await.unwrap();
    assert_eq!(hits(), before, "the catalog is read from the stored index");
    assert!(contract.channels["packages:list"].response.is_valid(&s), "{s}");
    let available = s["available"].as_array().unwrap();
    // Only extensions are cards; services and models show as their dependencies.
    assert!(available.iter().all(|e| e["kind"] == "extension"), "{s}");
    let echo = available
        .iter()
        .find(|e| e["name"] == "nano")
        .expect("echo listed");
    assert_eq!(echo["version"], "0.3.0");
    let deps = echo["dependencies"].as_array().unwrap();
    assert!(!deps.is_empty(), "{echo}");
    assert!(deps.iter().any(|d| d["direct"] == true));
    assert!(deps.iter().all(|d| d["version"].is_string()), "{echo}");
    let dep_bytes: u64 = deps.iter().map(|d| d["sizeBytes"].as_u64().unwrap()).sum();
    assert_eq!(
        echo["installSizeBytes"].as_u64().unwrap(),
        echo["sizeBytes"].as_u64().unwrap() + dep_bytes
    );
    // The bundled manifest's permissions for the first-party extension.
    assert_eq!(
        echo["permissions"],
        json!(["microphone", "screen-recording", "accessibility"])
    );

    // The plan's download equals the card's install size on an empty store (nothing shared yet).
    let plan = ipc
        .call("packages:plan", json!({"op": "install", "names": ["nano"]}))
        .await
        .unwrap();
    let total = plan["downloadBytes"].as_u64().unwrap();
    assert!(total > 0);
    ipc.call(
        "packages:apply",
        json!({"planSha256": plan["planSha256"], "downloadBytes": plan["downloadBytes"]}),
    )
    .await
    .unwrap();
    let mut seen = vec![];
    while let Ok((name, payload)) = rx.try_recv() {
        if name == "packages:progress" {
            assert!(qrtz_ipc::validate_event(&name, &payload).is_ok(), "{payload}");
            seen.push(payload);
        }
    }
    assert!(!seen.is_empty(), "progress was reported");
    assert!(seen.len() <= 102, "at most one per percent: {}", seen.len());
    let last = seen.last().unwrap();
    assert_eq!(last["doneBytes"], last["totalBytes"]);
    let done: Vec<u64> = seen.iter().map(|p| p["doneBytes"].as_u64().unwrap()).collect();
    assert!(done.windows(2).all(|w| w[0] <= w[1]), "{done:?}");
    let s = ipc.call("packages:list", Value::Null).await.unwrap();
    assert!(s["packages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "nano"));
}
