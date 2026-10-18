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
}
