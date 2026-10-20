#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0144 §6 gate (g): the four sideload freshness cases as unit cases of the legacy ADR 0146 decision
//! (`qrtz_modules::sideload::decide`). Gate (g) itself now runs on ADR 0157's installer
//! (`Installer::sideload` / `refresh` / `apply` on the publisher's signed output): `tools/qrtz-publish/tests/gate_g.rs`
//! (`cargo test -p qrtz-publish --test gate_g`), which also holds the freshness-exemption cases.
mod common;
use common::*;
use qrtz_modules::lock::Lockfile;
use qrtz_modules::sideload::{
    decide, recheck_after_fresh_list, Confirmation, Refusal, Request, SideloadOrigin,
};
use serde_json::json;

const NOW: u64 = 1_800_000_000_000;

fn echo() -> qrtz_modules::ModuleManifest {
    manifest("echo", "1.0.0", vec![data_dep("parakeet-tdt-v2", "2.0.0")])
}

fn confirmed(issued: u64) -> Confirmation {
    Confirmation {
        package_sha256: hash("echo.qrtzpkg"),
        revocation_issued_ms: Some(issued),
    }
}

#[test]
fn g1_expired_list_sideload_installs_after_the_age_prompt_and_is_marked_stale() {
    let stored = snapshot(5, NOW - 20 * DAY, json!([]));
    let c = candidate(&echo(), "echo.qrtzpkg");
    let ask = Request::Sideload {
        origin: SideloadOrigin::UserPickedFile,
        confirmation: None,
        carried: None,
    };
    let Err(Refusal::NeedsConfirmation(p)) = decide(ask, &c, &PASS, Some(&stored), &Lockfile::default(), NOW)
    else {
        panic!("no prompt")
    };
    assert_eq!(p.revocation_age_ms, Some(20 * DAY));
    assert!(p.revocation_expired);
    let y = confirmed(stored.issued_ms);
    let go = Request::Sideload {
        origin: SideloadOrigin::UserPickedFile,
        confirmation: Some(&y),
        carried: None,
    };
    let d = decide(go, &c, &PASS, Some(&stored), &Lockfile::default(), NOW).unwrap();
    assert!(d.next_lock.modules["echo"].stale_revocation);
}

#[test]
fn g2_a_sideload_the_stored_list_revokes_is_refused_without_override() {
    let stored = snapshot(
        5,
        NOW - 20 * DAY,
        json!([{"package": "echo", "versions": ["1.0.0"], "reason": "pulled"}]),
    );
    let y = confirmed(stored.issued_ms);
    let go = Request::Sideload {
        origin: SideloadOrigin::UserPickedFile,
        confirmation: Some(&y),
        carried: None,
    };
    let e = decide(
        go,
        &candidate(&echo(), "echo.qrtzpkg"),
        &PASS,
        Some(&stored),
        &Lockfile::default(),
        NOW,
    );
    assert!(matches!(e, Err(Refusal::Revoked { .. })), "{e:?}");
}

#[test]
fn g3_a_carried_snapshot_older_than_the_stored_one_is_refused_as_rollback() {
    let stored = snapshot(5, NOW - 20 * DAY, json!([]));
    let old = verified(&snapshot(3, NOW - 50 * DAY, json!([])));
    let y = confirmed(stored.issued_ms);
    let go = Request::Sideload {
        origin: SideloadOrigin::UserPickedFile,
        confirmation: Some(&y),
        carried: Some(old),
    };
    let e = decide(
        go,
        &candidate(&echo(), "echo.qrtzpkg"),
        &PASS,
        Some(&stored),
        &Lockfile::default(),
        NOW,
    );
    assert_eq!(
        e.unwrap_err(),
        Refusal::RevocationRollback {
            carried: 3,
            stored: 5
        }
    );
}

#[test]
fn g4_a_later_fresh_index_revoking_the_sideloaded_module_disables_it() {
    let stored = snapshot(5, NOW - 20 * DAY, json!([]));
    let y = confirmed(stored.issued_ms);
    let go = Request::Sideload {
        origin: SideloadOrigin::UserPickedFile,
        confirmation: Some(&y),
        carried: None,
    };
    let lock = decide(
        go,
        &candidate(&echo(), "echo.qrtzpkg"),
        &PASS,
        Some(&stored),
        &Lockfile::default(),
        NOW,
    )
    .unwrap()
    .next_lock;
    let fresh = verified(&snapshot(
        6,
        NOW + DAY,
        json!([{"package": "echo", "reason": "pulled"}]),
    ));
    let (after, notices) = recheck_after_fresh_list(&lock, &fresh, NOW + DAY);
    assert_eq!(after.modules["echo"].disabled.as_deref(), Some("pulled"));
    assert_eq!(notices.len(), 1);
}
