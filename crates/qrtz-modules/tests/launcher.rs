//! ADR 0146 §3: the launcher starts only switched-on, unrevoked, unchanged, resolvable modules, and checks code
//! before every launch.
mod common;
use common::*;
use qrtz_modules::launcher::{launch, plan, CodeVerifier, Installed, LaunchStep, Policy, Skip, Spawner};
use qrtz_modules::lock::Lockfile;
use qrtz_modules::settings::GlobalSettings;
use qrtz_modules::{sha256_hex, ModuleManifest};
use serde_json::json;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

struct World {
    ms: Vec<(ModuleManifest, String)>,
    lock: Lockfile,
}

fn world() -> World {
    let ms: Vec<ModuleManifest> = vec![
        manifest("echo", "1.0.0", vec![xpc_dep("voice", "1.4.0")]),
        manifest(
            "nano",
            "1.0.0",
            vec![xpc_dep("voice", "1.4.0"), data_dep("nemotron-560ms", "1.0.0")],
        ),
        manifest("ctrl", "1.0.0", vec![]),
    ];
    let mut lock = Lockfile::default();
    for m in &ms {
        lock = install(&lock, m);
    }
    World {
        ms: ms
            .into_iter()
            .map(|m| {
                let h = sha256_hex(&bytes(&m));
                (m, h)
            })
            .collect(),
        lock,
    }
}

fn run(w: &World, cfg: serde_json::Value, policy: Policy) -> qrtz_modules::launcher::Plan {
    plan(
        &GlobalSettings::from_config(&cfg),
        &w.lock,
        |id| {
            w.ms.iter().find(|(m, _)| m.id == id).map(|(m, h)| Installed {
                manifest: m,
                manifest_sha256: h,
                dir: PathBuf::from(format!("/data/modules/{id}/versions/1.0.0")),
            })
        },
        "0.2.0",
        Path::new("/data"),
        policy,
    )
}

fn skipped(p: &qrtz_modules::launcher::Plan, id: &str) -> Option<Skip> {
    p.skipped.iter().find(|(m, _)| m == id).map(|(_, s)| s.clone())
}

#[test]
fn every_installed_module_starts_with_its_services_listed_for_checking() {
    let p = run(&world(), json!({}), Policy::Release);
    assert_eq!(
        p.launch.iter().map(|s| s.module.as_str()).collect::<Vec<_>>(),
        ["echo", "nano", "ctrl"]
    );
    let echo = &p.launch[0];
    assert_eq!(
        echo.entry,
        PathBuf::from("/data/modules/echo/versions/1.0.0/qrtz-helper")
    );
    assert_eq!(echo.services.len(), 1);
    assert_eq!(
        echo.services[0].path,
        PathBuf::from("/data/packages/xpc/voice/1.4.0")
    );
    assert!(
        p.launch[1].services.len() == 1,
        "data packages aren't code-checked"
    );
}

#[test]
fn the_global_switches_control_every_module() {
    let w = world();
    let p = run(
        &w,
        json!({"modules": {"ctrl": {"enabled": false}}}),
        Policy::Release,
    );
    assert_eq!(skipped(&p, "ctrl"), Some(Skip::SwitchedOff));
    assert_eq!(p.launch.len(), 2);
    let p = run(&w, json!({"modules": {"enabled": false}}), Policy::Release);
    assert!(p.launch.is_empty());
}

#[test]
fn revoked_changed_unresolved_and_unsigned_modules_dont_start() {
    let mut w = world();
    w.lock.modules.get_mut("echo").unwrap().disabled = Some("pulled".into());
    w.ms[1].1 = hash("tampered");
    w.lock.modules.get_mut("ctrl").unwrap();
    let p = run(&w, json!({}), Policy::Release);
    assert_eq!(skipped(&p, "echo"), Some(Skip::Revoked("pulled".into())));
    assert_eq!(skipped(&p, "nano"), Some(Skip::ManifestChanged));

    let mut w = world();
    w.lock.packages.get_mut("xpc:voice").unwrap().clear();
    let p = run(&w, json!({}), Policy::Release);
    assert!(matches!(skipped(&p, "echo"), Some(Skip::Unresolved(_))));

    // The repo's placeholder identity runs in dev builds only.
    let mut ctrl = json!(manifest_json("ctrl", "1.0.0", vec![]));
    ctrl["code"]["teamId"] = json!(qrtz_modules::manifest::PLACEHOLDER_TEAM_ID);
    ctrl["code"]["designatedRequirement"] = json!(qrtz_modules::manifest::designated_requirement(
        "ai.qrtz.ctrl",
        qrtz_modules::manifest::PLACEHOLDER_TEAM_ID
    ));
    let ctrl = ModuleManifest::parse(ctrl.to_string().as_bytes()).unwrap();
    let mut w = world();
    w.lock = install(&w.lock, &ctrl);
    w.ms[2] = (ctrl.clone(), sha256_hex(&bytes(&ctrl)));
    assert_eq!(
        skipped(&run(&w, json!({}), Policy::Release), "ctrl"),
        Some(Skip::PlaceholderIdentity)
    );
    assert!(skipped(&run(&w, json!({}), Policy::Dev), "ctrl").is_none());
}

#[test]
fn an_app_outside_min_and_max_app_doesnt_start_the_module() {
    let mut v = manifest_json("ctrl", "1.0.0", vec![]);
    v["minApp"] = json!("0.3.0");
    let m = ModuleManifest::parse(v.to_string().as_bytes()).unwrap();
    let mut w = world();
    w.lock = install(&w.lock, &m);
    w.ms[2] = (m.clone(), sha256_hex(&bytes(&m)));
    assert_eq!(
        skipped(&run(&w, json!({}), Policy::Release), "ctrl"),
        Some(Skip::IncompatibleApp)
    );
}

struct Verifier(Vec<PathBuf>);
impl CodeVerifier for Verifier {
    fn verify(&self, path: &Path, req: &str) -> Result<(), String> {
        assert!(req.contains("anchor apple generic"));
        if self.0.iter().any(|p| p == path) {
            Err("requirement not satisfied".into())
        } else {
            Ok(())
        }
    }
}
#[derive(Default)]
struct Rec(RefCell<Vec<String>>);
impl Spawner for &Rec {
    fn spawn(&mut self, s: &LaunchStep) -> Result<(), String> {
        self.0.borrow_mut().push(s.module.clone());
        Ok(())
    }
}

#[test]
fn a_failed_code_check_on_the_module_or_its_service_never_reaches_the_spawner() {
    let p = run(&world(), json!({}), Policy::Release);
    let rec = Rec::default();
    let v = Verifier(vec![PathBuf::from("/data/packages/xpc/voice/1.4.0")]);
    let r = launch(&p, &v, &mut &rec);
    assert!(r[0].1.as_ref().unwrap_err().starts_with("xpc:voice"));
    assert!(r[1].1.is_err());
    assert!(r[2].1.is_ok());
    assert_eq!(*rec.0.borrow(), ["ctrl"]);
    let v = Verifier(vec![PathBuf::from(
        "/data/modules/ctrl/versions/1.0.0/qrtz-helper",
    )]);
    let rec = Rec::default();
    launch(&p, &v, &mut &rec);
    assert!(!rec.0.borrow().contains(&"ctrl".to_owned()));
}
