//! ctrl do's fixed action set (ADR 0214) and the host's policy over it. There is no free-form AppleScript, no
//! coordinate click, no global "type this string": every action is one of these, on an element handle the helper
//! itself created (from `ctrl.tree` or `ctrl.menu.shortcuts`), or a key combo from the app's own menu bar or the
//! built-in table, or opening an app or an http(s) address. The helper checks the same rules again before it acts.
use crate::ax::AxNode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDir {
    Up,
    Down,
}

/// One action, as logged: text is a pointer into the goal (`goal#5..12`, `site:goal#5..12`), resolved only for the
/// helper call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Press {
        handle: String,
    },
    /// Set the text field's value, then (submit) press Return in it.
    SetValue {
        handle: String,
        text_ref: String,
        submit: bool,
    },
    Increment {
        handle: String,
    },
    Decrement {
        handle: String,
    },
    ShowMenu {
        handle: String,
    },
    Focus {
        handle: String,
    },
    Scroll {
        handle: String,
        dir: ScrollDir,
    },
    /// To `app`, which ctrl brought to the front (foreground is allowed, ADR 0214).
    KeyCombo {
        app: String,
        combo: String,
    },
    OpenUrl {
        app: String,
        url_ref: String,
    },
    /// Launch or activate.
    OpenApp {
        app: String,
    },
}

impl Action {
    pub fn handle(&self) -> Option<&str> {
        match self {
            Action::Press { handle }
            | Action::SetValue { handle, .. }
            | Action::Increment { handle }
            | Action::Decrement { handle }
            | Action::ShowMenu { handle }
            | Action::Focus { handle }
            | Action::Scroll { handle, .. } => Some(handle),
            Action::KeyCombo { .. } | Action::OpenUrl { .. } | Action::OpenApp { .. } => None,
        }
    }

    /// The same action on another element (an approval re-bound after a re-read gave out fresh ids, review M1).
    pub fn with_handle(&self, h: &str) -> Action {
        let mut a = self.clone();
        match &mut a {
            Action::Press { handle }
            | Action::SetValue { handle, .. }
            | Action::Increment { handle }
            | Action::Decrement { handle }
            | Action::ShowMenu { handle }
            | Action::Focus { handle }
            | Action::Scroll { handle, .. } => *handle = h.to_string(),
            Action::KeyCombo { .. } | Action::OpenUrl { .. } | Action::OpenApp { .. } => {}
        }
        a
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Action::Press { .. } => "press",
            Action::SetValue { .. } => "set_value",
            Action::Increment { .. } => "increment",
            Action::Decrement { .. } => "decrement",
            Action::ShowMenu { .. } => "show_menu",
            Action::Focus { .. } => "focus",
            Action::Scroll { .. } => "scroll",
            Action::KeyCombo { .. } => "key_combo",
            Action::OpenUrl { .. } => "open_url",
            Action::OpenApp { .. } => "open_app",
        }
    }

    /// The text pointer, if the action carries one.
    pub fn text_ref(&self) -> Option<&str> {
        match self {
            Action::SetValue { text_ref, .. } => Some(text_ref),
            Action::OpenUrl { url_ref, .. } => Some(url_ref),
            _ => None,
        }
    }

    /// What an approval binds to besides the handle and the tree: the action and its parameters (refs, not text).
    pub fn binding_key(&self) -> String {
        match self {
            Action::SetValue { text_ref, submit, .. } => format!("set_value:{text_ref}:{submit}"),
            Action::Scroll { dir, .. } => format!("scroll:{dir:?}"),
            Action::KeyCombo { app, combo } => format!("key:{app}:{combo}"),
            Action::OpenUrl { app, url_ref } => format!("open_url:{app}:{url_ref}"),
            Action::OpenApp { app } => format!("open_app:{app}"),
            a => a.kind().to_string(),
        }
    }

    /// The `ctrl.ax.act` payload: `text` is the resolved pointer (this call only, never logged).
    pub fn wire(&self, text: Option<&str>) -> serde_json::Value {
        let mut v = serde_json::to_value(self).expect("serializes");
        if let Some(o) = v.as_object_mut() {
            o.remove("text_ref");
            o.remove("url_ref");
            if let Some(t) = text {
                o.insert(
                    if matches!(self, Action::OpenUrl { .. }) {
                        "url"
                    } else {
                        "text"
                    }
                    .into(),
                    t.into(),
                );
            }
        }
        v
    }
}

/// Why the host refused an action before sending it (short codes, logged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The handle isn't in the current tree (or menu) any more.
    Stale,
    /// The element doesn't offer that action.
    NotOffered,
    /// A combo outside the app's menu bar and the built-in table, or a reserved one (the stop key).
    ComboNotAllowed,
    /// Not an http(s) address.
    BadUrl,
    UnknownApp,
    /// The text is empty or too long.
    BadText,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::Stale => "act.stale",
            Refusal::NotOffered => "act.not_offered",
            Refusal::ComboNotAllowed => "act.combo_not_allowed",
            Refusal::BadUrl => "act.bad_url",
            Refusal::UnknownApp => "act.unknown_app",
            Refusal::BadText => "act.bad_text",
        }
    }
}

/// The built-in shortcut table (spec §3 `choose_move` source b): combo, readable label, browsers only.
pub const BUILTIN_SHORTCUTS: &[(&str, &str, bool)] = &[
    ("cmd+l", "focus the address bar", true),
    ("cmd+t", "new tab", true),
    ("cmd+f", "find", false),
    ("cmd+n", "new", false),
    ("cmd+comma", "settings", false),
];

/// Combos ctrl never sends: the stop key and force quit.
pub const RESERVED_COMBOS: &[&str] = &["escape", "cmd+option+escape"];

/// Canonical combo: lowercase, modifiers ordered cmd, ctrl, option, shift, then the key. Menu glyphs are spelled out.
pub fn normalize_combo(combo: &str) -> String {
    let mut s = combo.to_lowercase();
    for (g, w) in [("⌘", "cmd+"), ("⌃", "ctrl+"), ("⌥", "option+"), ("⇧", "shift+")] {
        s = s.replace(g, w);
    }
    let mut mods = [false; 4];
    let mut key = String::new();
    for raw in s.split('+') {
        match raw.trim() {
            "" => {}
            "cmd" | "command" | "meta" | "super" => mods[0] = true,
            "ctrl" | "control" => mods[1] = true,
            "option" | "opt" | "alt" => mods[2] = true,
            "shift" => mods[3] = true,
            "," => key = "comma".into(),
            "esc" => key = "escape".into(),
            "↩" | "enter" => key = "return".into(),
            k => key = k.into(),
        }
    }
    let names = ["cmd", "ctrl", "option", "shift"];
    names
        .iter()
        .zip(mods)
        .filter(|(_, m)| *m)
        .map(|(n, _)| n.to_string())
        .chain(std::iter::once(key))
        .collect::<Vec<_>>()
        .join("+")
}

/// Is `combo` one ctrl may send to an app whose menu bar offers `menu_combos`?
pub fn combo_allowed(combo: &str, menu_combos: &[String], browser: bool) -> bool {
    let c = normalize_combo(combo);
    if RESERVED_COMBOS.contains(&c.as_str()) || c.ends_with('+') || c.is_empty() {
        return false;
    }
    c == "return"
        || menu_combos.iter().any(|m| normalize_combo(m) == c)
        || BUILTIN_SHORTCUTS
            .iter()
            .any(|(b, _, only_browser)| *b == c && (browser || !only_browser))
}

/// Does the element offer the action?
pub fn offered(a: &Action, n: &AxNode) -> bool {
    let has = |x: &str| n.actions.iter().any(|a| a == x);
    match a {
        Action::Press { .. } => has("AXPress"),
        Action::Increment { .. } => has("AXIncrement"),
        Action::Decrement { .. } => has("AXDecrement"),
        Action::ShowMenu { .. } => has("AXShowMenu"),
        Action::Focus { .. } | Action::SetValue { .. } => n.text_input,
        Action::Scroll { .. } => matches!(
            n.role.as_str(),
            "AXScrollArea" | "AXList" | "AXTable" | "AXOutline" | "AXWebArea"
        ),
        _ => true,
    }
}

/// A resolved address ctrl may open or type into a browser's address bar as a URL.
pub fn http_url(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    if let Some((scheme, _)) = l.split_once("://") {
        return scheme == "http" || scheme == "https";
    }
    // A bare host ("youtube.com", "calendar.google.com/x"): no scheme, no spaces.
    !l.is_empty() && !l.contains(char::is_whitespace) && !l.contains(':')
}

/// Longest text ctrl types.
pub const TEXT_MAX: usize = 500;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_normalize_and_only_known_ones_pass() {
        assert_eq!(normalize_combo("⇧⌘N"), "cmd+shift+n");
        assert_eq!(normalize_combo("Shift+Cmd+n"), "cmd+shift+n");
        assert_eq!(normalize_combo("cmd+,"), "cmd+comma");
        let menu = vec!["cmd+shift+n".to_string()];
        assert!(combo_allowed("⇧⌘N", &menu, false));
        assert!(combo_allowed("cmd+l", &[], true));
        assert!(
            !combo_allowed("cmd+l", &[], false),
            "address bar is browsers only"
        );
        assert!(!combo_allowed("esc", &menu, true), "the stop key is reserved");
        assert!(!combo_allowed("cmd+option+esc", &[], true));
        assert!(!combo_allowed("cmd+q", &[], true), "not in this app's menu bar");
    }

    #[test]
    fn urls_are_http_only() {
        assert!(http_url("youtube.com") && http_url("https://apple.com/x"));
        assert!(!http_url("file:///etc/passwd") && !http_url("javascript:alert(1)"));
        assert!(!http_url("x-apple.systempreferences:com.apple") && !http_url("a b"));
    }

    #[test]
    fn the_wire_form_carries_text_only_for_the_call() {
        let a = Action::SetValue {
            handle: "e:1:2".into(),
            text_ref: "site:goal#5..12".into(),
            submit: true,
        };
        let v = a.wire(Some("youtube.com"));
        assert_eq!(v["action"], "set_value");
        assert_eq!(v["text"], "youtube.com");
        assert!(v.get("text_ref").is_none());
        assert!(!serde_json::to_string(&a).unwrap().contains("youtube"));
    }
}
