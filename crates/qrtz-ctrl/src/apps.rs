//! "Quit apps I haven't used today" (ADR 0130): which running apps count as unused, from the helper's last-used
//! signal. Pure: the helper reports every running regular app with the last time it saw that app become active
//! (`NSWorkspace.didActivateApplicationNotification`, kept in the helper's memory, never read from the screen), the
//! time it started watching, and the Mac's local midnight; the rule here decides.
//!
//! Cold start: an app with no recorded activation counts as unused only when the helper has watched since before
//! today's midnight without a break. On the first day (or after the helper restarted today) such an app is kept, so
//! nothing is quit just because ctrl wasn't watching. Never quit, whatever the record says: the frontmost app,
//! Finder, qrtz itself, and an app the user activated today.
use crate::helper::RunningAppWire;

/// The move of the quit-unused plan (behind the ADR 0111 gate, preview of the apps).
pub const QUIT_UNUSED_MOVE: &str = "route.workspace.quit_unused";

/// ADR 0148: the move of "quit <app>" (one named app, behind the gate, never the frontmost).
pub const QUIT_APP_MOVE: &str = "route.workspace.quit_app";

/// Never quit by "quit unused apps" (ctrl's own app, the Finder, the login / system shells).
pub const PROTECTED: &[&str] = &[
    "com.apple.finder",
    "ai.qrtz.desktop",
    "com.apple.loginwindow",
    "com.apple.dock",
    "com.apple.systemuiserver",
];

fn protected(id: &str) -> bool {
    PROTECTED.contains(&id) || id.starts_with("ai.qrtz.")
}

/// The running apps that count as unused today, in the helper's order. `since_ms`: when the helper's watch began
/// (`None`: it doesn't watch, nothing counts as unused). `day_start_ms`: local midnight today.
pub fn unused_today(
    apps: &[RunningAppWire],
    since_ms: Option<u64>,
    day_start_ms: u64,
) -> Vec<&RunningAppWire> {
    let Some(since) = since_ms else {
        return Vec::new();
    };
    let watched_all_day = since < day_start_ms;
    apps.iter()
        .filter(|a| !a.frontmost && !protected(&a.bundle_id))
        .filter(|a| match a.last_active_ms {
            Some(t) => t < day_start_ms && watched_all_day,
            None => watched_all_day,
        })
        .collect()
}

/// ADR 0135: the move of "mute my mac" (the system output, reversible, no gate, no UI).
pub const MUTE_MOVE: &str = "route.system.mute";
/// ADR 0139: "unmute my mac", the reverse of [`MUTE_MOVE`].
pub const UNMUTE_MOVE: &str = "route.system.unmute";
/// ADR 0140: "set the volume to N"; the level is the plan's `choose_target` option `level.N`.
pub const SET_VOLUME_MOVE: &str = "route.system.set_volume";
/// ADR 0140: the `choose_target` option prefix of a volume level (`level.0` … `level.100`).
pub const LEVEL_PREFIX: &str = "level.";

/// ADR 0140: the level a `level.N` option names, only 0–100.
pub fn level_of(opt: &str) -> Option<u8> {
    let n: u8 = opt.strip_prefix(LEVEL_PREFIX)?.parse().ok()?;
    (n <= 100).then_some(n)
}

/// ADR 0143: "turn it up" / "turn the volume down": a step from the level read just before (read-modify-write).
pub const VOLUME_UP_MOVE: &str = "route.system.volume_up";
pub const VOLUME_DOWN_MOVE: &str = "route.system.volume_down";
/// ADR 0143: the `choose_target` option prefix of a relative step (`step.1` … `step.100`).
pub const STEP_PREFIX: &str = "step.";
/// ADR 0143: the step when the task names none ("turn it up"); "by N" in the task overrides it.
pub const DEFAULT_VOLUME_STEP: u8 = 10;
/// ADR 0143: the step for "a bit" / "a little" / "slightly" with no number.
pub const SMALL_VOLUME_STEP: u8 = 5;

/// ADR 0143: the step a `step.N` option names, only 1–100.
pub fn step_of(opt: &str) -> Option<u8> {
    let n: u8 = opt.strip_prefix(STEP_PREFIX)?.parse().ok()?;
    (1..=100).contains(&n).then_some(n)
}

/// ADR 0143: the level a relative change aims at from the level read before, kept inside 0–100.
pub fn stepped(before: i64, step: u8, up: bool) -> u8 {
    let d = i64::from(step);
    let t = if up { before + d } else { before - d };
    t.clamp(0, 100) as u8
}

/// ADR 0134: the move of "hide other apps" / "hide all my apps" (reversible, no gate).
pub const HIDE_OTHERS_MOVE: &str = "route.workspace.hide_others";

/// ADR 0149: the move of "hide <app>" (one named app, reversible, no gate, never the frontmost).
pub const HIDE_APP_MOVE: &str = "route.workspace.hide_app";

/// ADR 0149: what "hide <app>" does with `id` now. `Err(())` holds it (the app the user is in: hiding it would bring
/// another app forward), `Ok(None)` is done with nothing to do (not running, or hidden already), `Ok(Some(id))` hides
/// it. An older helper that doesn't say whether the app is hidden (`hidden: None`) counts as visible.
#[allow(clippy::result_unit_err)]
pub fn hide_one(apps: &[RunningAppWire], id: &str) -> Result<Option<String>, ()> {
    match apps.iter().find(|a| a.bundle_id == id) {
        None => Ok(None),
        Some(a) if a.frontmost => Err(()),
        Some(a) if a.hidden == Some(true) => Ok(None),
        Some(a) => Ok(Some(a.bundle_id.clone())),
    }
}

/// Never hidden by "hide other apps": the frontmost app (the one the user is in) and qrtz itself, which shows the
/// run. The Finder is hidden like any other app (its windows go; the desktop stays), as the system's Hide Others does.
fn keep_visible(id: &str) -> bool {
    id.starts_with("ai.qrtz.")
        || matches!(
            id,
            "com.apple.loginwindow" | "com.apple.dock" | "com.apple.systemuiserver"
        )
}

/// The running apps "hide other apps" would hide now, in the helper's order: every regular app that isn't frontmost,
/// isn't qrtz, and isn't hidden already (`hidden: None`, an older helper that doesn't say, counts as visible).
pub fn to_hide(apps: &[RunningAppWire]) -> Vec<&RunningAppWire> {
    apps.iter()
        .filter(|a| !a.frontmost && !keep_visible(&a.bundle_id) && a.hidden != Some(true))
        .collect()
}

/// Of `planned`, how many are still on screen (running, not hidden, not frontmost: an app the user switched to
/// meanwhile is theirs to see). `None` when the helper can't say whether an app is hidden.
pub fn still_visible(apps: &[RunningAppWire], planned: &[String]) -> Option<i64> {
    let mut n = 0;
    for a in apps
        .iter()
        .filter(|a| planned.contains(&a.bundle_id) && !a.frontmost)
    {
        match a.hidden {
            Some(true) => {}
            Some(false) => n += 1,
            None => return None,
        }
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, last: Option<u64>, front: bool) -> RunningAppWire {
        RunningAppWire {
            bundle_id: id.into(),
            name: id.rsplit('.').next().unwrap().into(),
            last_active_ms: last,
            frontmost: front,
            unsaved: Some(false),
            hidden: Some(false),
        }
    }

    const MIDNIGHT: u64 = 1_000_000;

    #[test]
    fn the_rule_table() {
        let apps = vec![
            app("com.a.Used", Some(MIDNIGHT + 5), false),
            app("com.a.Yesterday", Some(MIDNIGHT - 5), false),
            app("com.a.Never", None, false),
            app("com.a.Front", None, true),
            app("com.apple.finder", None, false),
            app("ai.qrtz.desktop", None, false),
        ];
        let ids = |v: Vec<&RunningAppWire>| v.iter().map(|a| a.bundle_id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(unused_today(&apps, Some(MIDNIGHT - 100), MIDNIGHT)),
            vec!["com.a.Yesterday", "com.a.Never"]
        );
        // Cold start: the helper began watching today, so nothing counts as unused yet.
        assert!(unused_today(&apps, Some(MIDNIGHT + 1), MIDNIGHT).is_empty());
        // No watch at all: nothing.
        assert!(unused_today(&apps, None, MIDNIGHT).is_empty());
    }

    #[test]
    fn hide_others_keeps_the_frontmost_and_qrtz() {
        let mut apps = vec![
            app("com.a.Front", None, true),
            app("com.a.Slack", None, false),
            app("com.apple.finder", None, false),
            app("ai.qrtz.desktop", None, false),
            app("com.a.Hidden", None, false),
        ];
        apps[4].hidden = Some(true);
        let ids: Vec<String> = to_hide(&apps).iter().map(|a| a.bundle_id.clone()).collect();
        assert_eq!(ids, vec!["com.a.Slack", "com.apple.finder"]);
        assert_eq!(still_visible(&apps, &ids), Some(2));
        apps[1].hidden = Some(true);
        apps[2].hidden = Some(true);
        assert_eq!(still_visible(&apps, &ids), Some(0));
        // The user switched to Slack meanwhile: theirs, not counted.
        apps[1].hidden = Some(false);
        apps[1].frontmost = true;
        assert_eq!(still_visible(&apps, &ids), Some(0));
        apps[2].hidden = None;
        assert_eq!(still_visible(&apps, &ids), None);
    }

    #[test]
    fn a_relative_step_parses_only_1_to_100_and_clamps_the_target_to_the_range() {
        assert_eq!(step_of("step.10"), Some(10));
        assert_eq!(step_of("step.100"), Some(100));
        for bad in ["step.0", "step.101", "step.-5", "step.x", "level.10", "step."] {
            assert_eq!(step_of(bad), None, "{bad}");
        }
        assert_eq!(stepped(50, 10, true), 60);
        assert_eq!(stepped(50, 10, false), 40);
        assert_eq!(stepped(95, 10, true), 100);
        assert_eq!(stepped(3, 10, false), 0);
        assert_eq!(stepped(100, 5, true), 100);
        assert_eq!(stepped(0, 5, false), 0);
        // A read outside 0–100 (a broken report) still lands inside the range.
        assert_eq!(stepped(140, 10, false), 100);
        assert_eq!(stepped(-20, 10, true), 0);
    }
}
