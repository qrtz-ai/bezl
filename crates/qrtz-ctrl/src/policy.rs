//! The static never-takes-over-the-ui tables (ADR 0100). `native/qrtz-helper/Fixtures/ctrl/policy.json` mirrors
//! them; tests on both the Rust and the Swift side fail if either drifts from the fixture.

/// AX actions that raise windows, open menus (modal mouse tracking) or pick menu items. Never sent.
pub const FORBIDDEN_AX_ACTIONS: &[&str] = &["AXRaise", "AXShowMenu", "AXPick"];

/// AX attributes whose write activates an app, moves focus, or moves/resizes a window on the user's screen.
/// ADR 0187: `AXMinimized` too, both ways, until a Mac probe shows the write raises nothing: un-minimizing puts a
/// window back on the user's screen (AppKit's deminiaturize orders it front), minimizing takes one off it.
pub const FORBIDDEN_AX_SET_ATTRIBUTES: &[&str] = &[
    "AXFocused",
    "AXMain",
    "AXFrontmost",
    "AXPosition",
    "AXSize",
    "AXFullScreen",
    "AXMinimized",
];

/// Roles where `AXPress` opens a menu (which grabs mouse tracking) or drives the menu bar, which always belongs to
/// the frontmost app; a background app's menu items are also routinely disabled.
pub const MENU_OPENING_ROLES: &[&str] = &[
    "AXMenuBar",
    "AXMenuBarItem",
    "AXMenuItem",
    "AXMenuButton",
    "AXPopUpButton",
    "AXComboBox",
];

/// Roles ctrl never reads, writes or types into.
pub const SECURE_ROLES: &[&str] = &["AXSecureTextField"];

/// Shortcuts that mean "put my cursor here" or switch what the user sees (tabs, apps, Spaces). Even posted to a
/// background pid the target treats them as activation intent or flips its visible content.
pub const FOCUS_INTENT_SHORTCUTS: &[&str] = &[
    "cmd+l",
    "cmd+shift+g",
    "cmd+tab",
    "cmd+grave",
    "cmd+space",
    "cmd+1",
    "cmd+2",
    "cmd+3",
    "cmd+4",
    "cmd+5",
    "cmd+6",
    "cmd+7",
    "cmd+8",
    "cmd+9",
    "cmd+[",
    "cmd+]",
    "cmd+shift+[",
    "cmd+shift+]",
    "ctrl+tab",
    "ctrl+shift+tab",
    "ctrl+left",
    "ctrl+right",
    "ctrl+up",
    "ctrl+down",
];

/// The user counts as active in an app if their last physical input was this recent; ctrl defers instead of
/// acting in the app they are using.
pub const USER_ACTIVE_WINDOW_MS: u64 = 2000;

/// Cursor moves smaller than this (points, per axis) are rounding, not a move.
pub const CURSOR_TOLERANCE_PT: f64 = 0.5;

/// Canonical form of a key combo: lowercase, modifiers in a fixed order (cmd, ctrl, option, shift), then the key.
pub fn normalize_combo(combo: &str) -> String {
    let mut mods = [false; 4];
    let mut key = String::new();
    // Menu glyphs ("⌘⇧G") carry no separator; spell them out first.
    let spelled = combo
        .to_lowercase()
        .replace('⌘', "cmd+")
        .replace('⌃', "ctrl+")
        .replace('⌥', "option+")
        .replace('⇧', "shift+");
    for part in spelled.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part {
            "cmd" | "command" | "meta" | "super" => mods[0] = true,
            "ctrl" | "control" => mods[1] = true,
            "option" | "opt" | "alt" => mods[2] = true,
            "shift" => mods[3] = true,
            "`" => key = "grave".into(),
            k => key = k.to_string(),
        }
    }
    let names = ["cmd", "ctrl", "option", "shift"];
    let mut out: Vec<&str> = names
        .iter()
        .zip(mods)
        .filter(|(_, on)| *on)
        .map(|(n, _)| *n)
        .collect();
    out.push(&key);
    out.join("+")
}

pub fn is_focus_intent(combo: &str) -> bool {
    let c = normalize_combo(combo);
    FOCUS_INTENT_SHORTCUTS.iter().any(|s| normalize_combo(s) == c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../native/qrtz-helper/Fixtures/ctrl/policy.json"
        );
        serde_json::from_str(&std::fs::read_to_string(p).expect("policy fixture")).expect("json")
    }

    fn list(v: &Value, k: &str) -> Vec<String> {
        v[k].as_array()
            .unwrap_or_else(|| panic!("{k}"))
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    }

    fn own(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tables_match_the_shared_fixture() {
        let f = fixture();
        assert_eq!(list(&f, "forbidden_ax_actions"), own(FORBIDDEN_AX_ACTIONS));
        assert_eq!(
            list(&f, "forbidden_ax_set_attributes"),
            own(FORBIDDEN_AX_SET_ATTRIBUTES)
        );
        assert_eq!(list(&f, "menu_opening_roles"), own(MENU_OPENING_ROLES));
        assert_eq!(list(&f, "secure_roles"), own(SECURE_ROLES));
        assert_eq!(list(&f, "focus_intent_shortcuts"), own(FOCUS_INTENT_SHORTCUTS));
        assert_eq!(f["user_active_window_ms"].as_u64(), Some(USER_ACTIVE_WINDOW_MS));
        assert_eq!(f["cursor_tolerance_pt"].as_f64(), Some(CURSOR_TOLERANCE_PT));
    }

    #[test]
    fn combos_normalize() {
        assert_eq!(normalize_combo("Shift+Cmd+G"), "cmd+shift+g");
        assert_eq!(normalize_combo("⌘L"), "cmd+l");
        assert_eq!(normalize_combo("⌘⇧G"), "cmd+shift+g");
        assert!(is_focus_intent("⌘L"));
        assert_eq!(normalize_combo("command+l"), "cmd+l");
        assert!(is_focus_intent("CMD+L"));
        assert!(is_focus_intent("shift+cmd+]"));
        assert!(!is_focus_intent("cmd+w"));
        assert!(!is_focus_intent("cmd+c"));
    }
}
