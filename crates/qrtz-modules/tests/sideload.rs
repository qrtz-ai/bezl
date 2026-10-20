#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Bozniak's sideload rules (ADR 0146 §5). Rules 2, 3 and 6 also get gate tests on the confinement track; these
//! are the shell's own.
mod common;
use common::*;
use qrtz_modules::lock::{Lockfile, Source};
use qrtz_modules::sideload::{
    adopt_snapshot, decide, recheck_after_fresh_list, Confirmation, Refusal, Request, SideloadOrigin,
    VerifiedSnapshot,
};
use serde_json::json;

const NOW: u64 = 1_800_000_000_000;

fn echo() -> qrtz_modules::ModuleManifest {
    manifest("echo", "1.0.0", vec![data_dep("parakeet-tdt-v2", "2.0.0")])
}

fn yes(file: &str, issued: Option<u64>) -> Confirmation {
    Confirmation {
        package_sha256: hash(file),
        revocation_issued_ms: issued,
    }
}

fn sideload<'a>(c: Option<&'a Confirmation>, carried: Option<VerifiedSnapshot>) -> Request<'a> {
    Request::Sideload {
        origin: SideloadOrigin::UserPickedFile,
        confirmation: c,
        carried,
    }
}

#[test]
fn rule1_network_refuses_an_expired_or_missing_list() {
    let c = candidate(&echo(), "pkg");
    let old = snapshot(5, NOW - 9 * DAY, json!([]));
    for stored in [None, Some(&old)] {
        assert_eq!(
            decide(Request::Network, &c, &PASS, stored, &Lockfile::default(), NOW).unwrap_err(),
            Refusal::RevocationListExpired
        );
    }
    let fresh = snapshot(6, NOW - DAY, json!([]));
    let d = decide(
        Request::Network,
        &c,
        &PASS,
        Some(&fresh),
        &Lockfile::default(),
        NOW,
    )
    .unwrap();
    assert!(!d.stale_revocation);
    assert_eq!(d.next_lock.modules["echo"].source, Source::Network);
}

#[test]
fn rule2_a_sideload_with_an_expired_list_is_checked_against_the_stored_one() {
    let c = candidate(&echo(), "pkg");
    let stored = snapshot(5, NOW - 30 * DAY, json!([]));
    let y = yes("pkg", Some(stored.issued_ms));
    let d = decide(
        sideload(Some(&y), None),
        &c,
        &PASS,
        Some(&stored),
        &Lockfile::default(),
        NOW,
    )
    .unwrap();
    assert!(d.stale_revocation);
    assert!(d.next_lock.modules["echo"].stale_revocation);
    assert_eq!(d.next_lock.modules["echo"].source, Source::Sideload);
    // Revoked in the stored list: refused, module or package, whatever version form.
    for revoked in [
        json!([{"package": "echo", "versions": ["1.0.0"], "reason": "bad build"}]),
        json!([{"package": "echo", "reason": "all versions"}]),
        json!([{"package": "model:parakeet-tdt-v2", "versions": ["2.0.0"], "reason": "bad weights"}]),
    ] {
        let stored = snapshot(5, NOW - 30 * DAY, revoked);
        let y = yes("pkg", Some(stored.issued_ms));
        assert!(matches!(
            decide(
                sideload(Some(&y), None),
                &c,
                &PASS,
                Some(&stored),
                &Lockfile::default(),
                NOW
            ),
            Err(Refusal::Revoked { .. })
        ));
    }
    // Revoking another version doesn't touch this one.
    let stored = snapshot(
        5,
        NOW - 30 * DAY,
        json!([{"package": "echo", "versions": ["0.9.0"], "reason": "x"}]),
    );
    let y = yes("pkg", Some(stored.issued_ms));
    assert!(decide(
        sideload(Some(&y), None),
        &c,
        &PASS,
        Some(&stored),
        &Lockfile::default(),
        NOW
    )
    .is_ok());
}

#[test]
fn rule3_a_carried_snapshot_is_adopted_only_if_newer() {
    let stored = snapshot(5, NOW - 30 * DAY, json!([]));
    let newer = verified(&snapshot(6, NOW - 2 * DAY, json!([])));
    let older = verified(&snapshot(4, NOW - 60 * DAY, json!([])));
    let same = verified(&snapshot(
        5,
        NOW - 30 * DAY,
        json!([{"package": "echo", "reason": "x"}]),
    ));
    let backdated = verified(&snapshot(7, NOW - 40 * DAY, json!([])));
    assert_eq!(
        adopt_snapshot(Some(&stored), Some(&newer)),
        (Some(newer.get().clone()), true)
    );
    assert_eq!(
        adopt_snapshot(Some(&stored), Some(&older)),
        (Some(stored.clone()), false)
    );
    assert_eq!(
        adopt_snapshot(Some(&stored), Some(&same)),
        (Some(stored.clone()), false)
    );
    assert_eq!(
        adopt_snapshot(Some(&stored), Some(&backdated)),
        (Some(stored.clone()), false)
    );
    assert_eq!(
        adopt_snapshot(None, Some(&older)),
        (Some(older.get().clone()), true)
    );

    // Through decide: a carried newer list that revokes the module refuses it and is still what the prompt shows.
    let c = candidate(&echo(), "pkg");
    let revoking = verified(&snapshot(
        6,
        NOW - DAY,
        json!([{"package": "echo", "reason": "pulled"}]),
    ));
    let y = yes("pkg", Some(NOW - DAY));
    assert!(matches!(
        decide(
            sideload(Some(&y), Some(revoking)),
            &c,
            &PASS,
            Some(&stored),
            &Lockfile::default(),
            NOW
        ),
        Err(Refusal::Revoked { .. })
    ));
    // A carried fresh list is adopted, the install isn't stale, and the caller gets it to store.
    let fresh = verified(&snapshot(6, NOW - DAY, json!([])));
    let d = decide(
        sideload(Some(&y), Some(fresh.clone())),
        &c,
        &PASS,
        Some(&stored),
        &Lockfile::default(),
        NOW,
    )
    .unwrap();
    assert_eq!(d.adopted.as_ref(), Some(fresh.get()));
    assert!(!d.stale_revocation);
    // An older carried list is a rollback: refused, so it can't hide a revocation in the stored one.
    let revoked_stored = snapshot(
        5,
        NOW - 30 * DAY,
        json!([{"package": "echo", "reason": "pulled"}]),
    );
    let y = yes("pkg", Some(revoked_stored.issued_ms));
    assert_eq!(
        decide(
            sideload(Some(&y), Some(older)),
            &c,
            &PASS,
            Some(&revoked_stored),
            &Lockfile::default(),
            NOW
        )
        .unwrap_err(),
        Refusal::RevocationRollback {
            carried: 4,
            stored: 5
        }
    );
    // So is a higher version issued before the stored one (a re-numbered old list).
    let y = yes("pkg", Some(stored.issued_ms));
    assert!(matches!(
        decide(
            sideload(Some(&y), Some(backdated)),
            &c,
            &PASS,
            Some(&stored),
            &Lockfile::default(),
            NOW
        ),
        Err(Refusal::RevocationRollback { .. })
    ));
    // An unsigned snapshot never becomes a VerifiedSnapshot.
    let bytes = serde_json::to_vec(&snapshot(9, NOW, json!([]))).unwrap();
    assert!(matches!(
        VerifiedSnapshot::verify(&FakeVerifier, &bytes, b"forged"),
        Err(Refusal::BadRevocationSignature(_))
    ));
}

#[test]
fn rule4_explicit_confirmation_of_this_file_showing_the_lists_age() {
    let c = candidate(&echo(), "pkg");
    let stored = snapshot(5, NOW - 30 * DAY, json!([]));
    match decide(
        sideload(None, None),
        &c,
        &PASS,
        Some(&stored),
        &Lockfile::default(),
        NOW,
    ) {
        Err(Refusal::NeedsConfirmation(p)) => {
            assert_eq!(p.revocation_age_ms, Some(30 * DAY));
            assert!(p.revocation_expired);
            assert_eq!(p.package_sha256, hash("pkg"));
        }
        other => panic!("{other:?}"),
    }
    match decide(sideload(None, None), &c, &PASS, None, &Lockfile::default(), NOW) {
        Err(Refusal::NeedsConfirmation(p)) => assert_eq!(p.revocation_age_ms, None),
        other => panic!("{other:?}"),
    }
    // A yes for another file, or for the age before a newer carried list, doesn't count.
    for y in [
        yes("other", Some(stored.issued_ms)),
        yes("pkg", None),
        yes("pkg", Some(NOW)),
    ] {
        assert_eq!(
            decide(
                sideload(Some(&y), None),
                &c,
                &PASS,
                Some(&stored),
                &Lockfile::default(),
                NOW
            )
            .unwrap_err(),
            Refusal::ConfirmationMismatch
        );
    }
    let y = yes("pkg", Some(stored.issued_ms));
    let watched = Request::Sideload {
        origin: SideloadOrigin::WatchedFolder,
        confirmation: Some(&y),
        carried: None,
    };
    assert_eq!(
        decide(watched, &c, &PASS, Some(&stored), &Lockfile::default(), NOW).unwrap_err(),
        Refusal::NotUserInitiated
    );
}

#[test]
fn rule5_a_sideload_installs_exactly_the_file_against_the_lockfile() {
    let stored = snapshot(5, NOW - 30 * DAY, json!([]));
    let y = yes("pkg", Some(stored.issued_ms));
    // Missing a pinned package: refused (nothing is fetched to fill it).
    let mut c = candidate(&echo(), "pkg");
    c.packages.clear();
    assert!(matches!(
        decide(
            sideload(Some(&y), None),
            &c,
            &PASS,
            Some(&stored),
            &Lockfile::default(),
            NOW
        ),
        Err(Refusal::Lock(_))
    ));
    // A package already installed with other bytes: refused.
    let nano = manifest("nano", "1.0.0", vec![data_dep("parakeet-tdt-v2", "2.0.0")]);
    let lock = install(&Lockfile::default(), &nano);
    let mut c = candidate(&echo(), "pkg");
    c.packages[0].1.sha256 = hash("evil");
    c.manifest.dependencies[0].sha256 = hash("evil");
    assert!(matches!(
        decide(sideload(Some(&y), None), &c, &PASS, Some(&stored), &lock, NOW),
        Err(Refusal::Lock(_))
    ));
    // The same bytes already installed: shared, not duplicated.
    let c = candidate(&echo(), "pkg");
    let d = decide(sideload(Some(&y), None), &c, &PASS, Some(&stored), &lock, NOW).unwrap();
    assert_eq!(d.next_lock.packages["model:parakeet-tdt-v2"].len(), 1);
    // Never below what is installed.
    let newer = manifest("echo", "2.0.0", vec![data_dep("parakeet-tdt-v2", "2.0.0")]);
    let lock = install(&Lockfile::default(), &newer);
    assert!(matches!(
        decide(sideload(Some(&y), None), &c, &PASS, Some(&stored), &lock, NOW),
        Err(Refusal::Downgrade { .. })
    ));
}

#[test]
fn rule6_stale_installs_are_rechecked_and_revoked_ones_disabled_with_a_notice() {
    let stored = snapshot(5, NOW - 30 * DAY, json!([]));
    let y = yes("pkg", Some(stored.issued_ms));
    let c = candidate(&echo(), "pkg");
    let lock = decide(
        sideload(Some(&y), None),
        &c,
        &PASS,
        Some(&stored),
        &Lockfile::default(),
        NOW,
    )
    .unwrap()
    .next_lock;
    let lock = install(&lock, &manifest("nano", "1.0.0", vec![]));
    // A fresh list with nothing revoked clears the mark.
    let (clean, n) = recheck_after_fresh_list(&lock, &verified(&snapshot(6, NOW, json!([]))), NOW);
    assert!(n.is_empty());
    assert!(!clean.modules["echo"].stale_revocation);
    // An expired "fresh" list changes nothing.
    let (same, n) = recheck_after_fresh_list(
        &lock,
        &verified(&snapshot(
            6,
            NOW - 9 * DAY,
            json!([{"package": "echo", "reason": "x"}]),
        )),
        NOW,
    );
    assert!(n.is_empty());
    assert_eq!(same, lock);
    // A fresh list revoking echo's model disables echo, once, and leaves nano alone.
    let fresh = verified(&snapshot(
        6,
        NOW,
        json!([{"package": "model:parakeet-tdt-v2", "versions": ["2.0.0"], "reason": "bad weights"}]),
    ));
    let (after, n) = recheck_after_fresh_list(&lock, &fresh, NOW);
    assert_eq!(n.len(), 1);
    assert_eq!(
        (n[0].module.as_str(), n[0].revoked.as_str()),
        ("echo", "model:parakeet-tdt-v2")
    );
    assert_eq!(after.modules["echo"].disabled.as_deref(), Some("bad weights"));
    assert!(after.modules["nano"].disabled.is_none());
    let (_, again) = recheck_after_fresh_list(&after, &fresh, NOW);
    assert!(again.is_empty(), "notified once");
}

#[test]
fn rule7_the_same_checks_run_on_both_paths_first() {
    let c = candidate(&echo(), "pkg");
    let fail = Checks(Err("team id".into()));
    let stored = snapshot(5, NOW - DAY, json!([]));
    let y = yes("pkg", Some(stored.issued_ms));
    assert_eq!(
        decide(
            Request::Network,
            &c,
            &fail,
            Some(&stored),
            &Lockfile::default(),
            NOW
        )
        .unwrap_err(),
        Refusal::ChecksFailed("team id".into())
    );
    assert_eq!(
        decide(
            sideload(Some(&y), None),
            &c,
            &fail,
            Some(&stored),
            &Lockfile::default(),
            NOW
        )
        .unwrap_err(),
        Refusal::ChecksFailed("team id".into())
    );
}
