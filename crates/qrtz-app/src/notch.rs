//! ADR 0218: the host's side of the notch surface. Only the host drives it: the helper takes `notch.*` only from
//! its host stdin, and modules never reach the helper; they ask the host, and [`NotchBroker`] enforces the rules
//! before a line goes out.
//!
//! - Owner and priority are host-assigned from who is asking ([`Requester`]), never from the request:
//!   system/dictation (echo fn) > ctrl run status > module requests. A higher priority preempts (the old module
//!   holder is dismissed); a lower one is refused with a reason while a higher owner holds it. Only the current
//!   owner can update or dismiss.
//! - Module text is plain text: control, newline and bidi characters are stripped, it is capped at 60 characters
//!   for a label and 120 for status, links are refused, icons come from the fixed built-in set by name, and a
//!   module can't draw the mic waveform (that is dictation's).
//! - Attribution: module content carries the module's registered display name (`source`, from its manifest), and
//!   module text may not start with `qrtz`, `ctrl`, `echo` or a system-sounding prefix.
//! - No clickable actions in v1 (the surface ignores the mouse).
//! - Module updates are rate-limited (10 a second, extras dropped) and module content is dismissed after 10 s
//!   without an update. Nothing is shown for a module while secure input is on (a password prompt is up).
//!   Dictation is exempt from the module rules; ctrl's text is sanitized and capped but may say `ctrl: …`.
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};

pub const LABEL_MAX: usize = 60;
pub const STATUS_MAX: usize = 120;
pub const SOURCE_MAX: usize = 40;
pub const MODULE_RATE_PER_SEC: usize = 10;
pub const MODULE_TIMEOUT_MS: u64 = 10_000;
pub const ICONS: &[&str] = &[
    "check", "alert", "info", "mic", "clock", "download", "bolt", "calendar",
];
/// Text a module may not start with (case-insensitive, after trimming): it would read as qrtz itself.
pub const RESERVED_PREFIXES: &[&str] = &[
    "qrtz",
    "ctrl",
    "echo",
    "dictation",
    "system",
    "macos",
    "mac os",
    "apple",
    "security",
    "password",
    "warning:",
    "alert:",
    "error:",
];

pub const CTRL_OWNER: &str = "ctrl.run";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Module = 1,
    Ctrl = 2,
    System = 3,
}

impl Priority {
    pub fn wire(self) -> &'static str {
        match self {
            Priority::Module => "module",
            Priority::Ctrl => "ctrl",
            Priority::System => "system",
        }
    }
}

/// Who asks. The host knows this from the call's origin (a module's IPC call carries its verified id); nothing in
/// the request can change it.
#[derive(Clone, Debug)]
pub enum Requester<'a> {
    Ctrl,
    /// `display_name`: the module's registered name from its manifest.
    Module {
        id: &'a str,
        display_name: &'a str,
    },
}

/// What a requester may ask the surface to show (the module-facing shape).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum NotchRequest {
    Level {
        busy: bool,
    },
    Status {
        text: String,
        #[serde(default)]
        error: bool,
    },
    Progress {
        label: String,
        #[serde(default)]
        value: Option<f64>,
    },
    Icon {
        icon: String,
        label: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A higher (or another equal) owner holds the surface.
    Busy {
        holder: String,
    },
    /// Only the current owner can update or dismiss.
    NotOwner,
    BadContent(&'static str),
    /// The text starts with a reserved prefix (impersonation).
    ReservedPrefix,
    RateLimited,
    /// Secure input is on: nothing is shown for a module.
    SecureInput,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Busy { holder } => write!(f, "the notch is in use by {holder}"),
            Refusal::NotOwner => f.write_str("only the notch's current owner can change it"),
            Refusal::BadContent(why) => write!(f, "notch content refused: {why}"),
            Refusal::ReservedPrefix => {
                f.write_str("notch text may not start like qrtz, ctrl, echo or the system")
            }
            Refusal::RateLimited => f.write_str("too many notch updates; this one was dropped"),
            Refusal::SecureInput => f.write_str("secure input is on; the notch shows nothing for modules"),
        }
    }
}

#[derive(Clone, Debug)]
struct Holder {
    owner: String,
    priority: Priority,
    last_ms: u64,
}

/// The host's notch state. Pure (time and secure input are passed in), so the shell and the tests use one rule.
/// Each call returns the helper lines to send, in order.
#[derive(Debug, Default)]
pub struct NotchBroker {
    holder: Option<Holder>,
    dictation: bool,
    rates: HashMap<String, VecDeque<u64>>,
}

/// Strips control (incl. newlines), bidi and zero-width direction characters, collapses whitespace and trims.
pub fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .filter(|c| {
            !c.is_control()
                && !matches!(*c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Caps at `max` UTF-16 units (the helper's wire check), ending with an ellipsis when cut.
pub fn cap(s: &str, max: usize) -> String {
    if s.encode_utf16().count() <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut n = 0;
    for c in s.chars() {
        if n + c.len_utf16() > max - 1 {
            break;
        }
        n += c.len_utf16();
        out.push(c);
    }
    out.trim_end().to_owned() + "…"
}

fn has_link(s: &str) -> bool {
    let l = s.to_lowercase();
    l.contains("://") || l.contains("www.") || l.contains("mailto:")
}

fn reserved(s: &str) -> bool {
    let l = s.trim_start().to_lowercase();
    RESERVED_PREFIXES.iter().any(|p| l.starts_with(p))
}

/// A module id as a wire owner: `module.<id>` with only [a-z0-9._-].
pub fn module_owner(id: &str) -> String {
    let id: String = id
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(56)
        .collect();
    format!("module.{id}")
}

/// The wire content for a request, sanitized and checked for this requester.
pub fn content(req: &NotchRequest, module: bool) -> Result<Value, Refusal> {
    let text = |s: &str, max: usize| -> Result<String, Refusal> {
        let t = sanitize(s);
        if t.is_empty() {
            return Err(Refusal::BadContent("empty text"));
        }
        if module && has_link(&t) {
            return Err(Refusal::BadContent("links aren't allowed"));
        }
        if module && reserved(&t) {
            return Err(Refusal::ReservedPrefix);
        }
        Ok(cap(&t, max))
    };
    Ok(match req {
        NotchRequest::Level { busy } => {
            if module {
                return Err(Refusal::BadContent("the waveform is dictation's"));
            }
            json!({"kind": "level", "busy": busy})
        }
        NotchRequest::Status { text: t, error } => {
            json!({"kind": "status", "text": text(t, STATUS_MAX)?, "tone": if *error { "error" } else { "normal" }})
        }
        NotchRequest::Progress { label, value } => {
            let mut o = json!({"kind": "progress", "label": text(label, LABEL_MAX)?});
            if let Some(v) = value {
                if !v.is_finite() || !(0.0..=1.0).contains(v) {
                    return Err(Refusal::BadContent("progress is 0 to 1"));
                }
                o["value"] = json!(v);
            }
            o
        }
        NotchRequest::Icon { icon, label } => {
            if !ICONS.contains(&icon.as_str()) {
                return Err(Refusal::BadContent("unknown icon"));
            }
            json!({"kind": "icon", "icon": icon, "label": text(label, LABEL_MAX)?})
        }
    })
}

impl NotchBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Who holds the surface now (the helper's own dictation counts as `echo.dictation`).
    pub fn holder(&self) -> Option<&str> {
        if self.dictation {
            return Some("echo.dictation");
        }
        self.holder.as_ref().map(|h| h.owner.as_str())
    }

    /// Echo's dictation (fn) began or ended; the helper draws it itself, above everything. A module holder is
    /// preempted (dismissed); ctrl's status stays underneath and comes back after.
    pub fn set_dictation(&mut self, active: bool) -> Vec<Value> {
        self.dictation = active;
        if active
            && self
                .holder
                .as_ref()
                .is_some_and(|h| h.priority == Priority::Module)
        {
            let h = self.holder.take().expect("checked");
            return vec![dismiss(&h.owner)];
        }
        Vec::new()
    }

    fn who(r: &Requester) -> (String, Priority, Option<String>) {
        match r {
            Requester::Ctrl => (CTRL_OWNER.to_owned(), Priority::Ctrl, None),
            Requester::Module { id, display_name } => {
                let name = cap(&sanitize(display_name), SOURCE_MAX);
                let name = if name.is_empty() {
                    cap(&sanitize(id), SOURCE_MAX)
                } else {
                    name
                };
                (module_owner(id), Priority::Module, Some(name))
            }
        }
    }

    fn rate_ok(&mut self, owner: &str, now_ms: u64) -> bool {
        let q = self.rates.entry(owner.to_owned()).or_default();
        while q.front().is_some_and(|t| now_ms.saturating_sub(*t) >= 1_000) {
            q.pop_front();
        }
        if q.len() >= MODULE_RATE_PER_SEC {
            return false;
        }
        q.push_back(now_ms);
        true
    }

    /// Show (or replace) this requester's content.
    pub fn show(
        &mut self,
        r: &Requester,
        req: &NotchRequest,
        now_ms: u64,
        secure_input: bool,
    ) -> Result<Vec<Value>, Refusal> {
        let (owner, priority, source) = Self::who(r);
        let module = priority == Priority::Module;
        if module && secure_input {
            return Err(Refusal::SecureInput);
        }
        if self.dictation {
            return Err(Refusal::Busy {
                holder: "echo.dictation".into(),
            });
        }
        let c = content(req, module)?;
        let mut out = Vec::new();
        if let Some(h) = &self.holder {
            if h.owner != owner {
                // Equal priority: first come keeps it; higher preempts.
                if h.priority >= priority {
                    return Err(Refusal::Busy {
                        holder: h.owner.clone(),
                    });
                }
                out.push(dismiss(&h.owner));
            }
        }
        if module && !self.rate_ok(&owner, now_ms) {
            return Err(Refusal::RateLimited);
        }
        let mut line =
            json!({"type": "notch.show", "owner": owner, "priority": priority.wire(), "content": c});
        if let Some(s) = source {
            line["source"] = json!(s);
        }
        out.push(line);
        self.holder = Some(Holder {
            owner,
            priority,
            last_ms: now_ms,
        });
        Ok(out)
    }

    /// Change the holder's content (refused for anyone else).
    pub fn update(
        &mut self,
        r: &Requester,
        req: &NotchRequest,
        now_ms: u64,
        secure_input: bool,
    ) -> Result<Vec<Value>, Refusal> {
        let (owner, priority, _) = Self::who(r);
        let module = priority == Priority::Module;
        if self.holder.as_ref().is_none_or(|h| h.owner != owner) {
            return Err(Refusal::NotOwner);
        }
        if module && secure_input {
            return Err(Refusal::SecureInput);
        }
        let c = content(req, module)?;
        if module && !self.rate_ok(&owner, now_ms) {
            return Err(Refusal::RateLimited);
        }
        if let Some(h) = self.holder.as_mut() {
            h.last_ms = now_ms;
        }
        Ok(vec![
            json!({"type": "notch.update", "owner": owner, "content": c}),
        ])
    }

    pub fn dismiss(&mut self, r: &Requester) -> Result<Vec<Value>, Refusal> {
        let (owner, _, _) = Self::who(r);
        if self.holder.as_ref().is_none_or(|h| h.owner != owner) {
            return Err(Refusal::NotOwner);
        }
        self.holder = None;
        Ok(vec![dismiss(&owner)])
    }

    /// Call about once a second: module content goes after 10 s without an update, or at once when secure input
    /// turns on.
    pub fn tick(&mut self, now_ms: u64, secure_input: bool) -> Vec<Value> {
        let expired = self.holder.as_ref().is_some_and(|h| {
            h.priority == Priority::Module
                && (secure_input || now_ms.saturating_sub(h.last_ms) >= MODULE_TIMEOUT_MS)
        });
        if expired {
            let h = self.holder.take().expect("checked");
            return vec![dismiss(&h.owner)];
        }
        Vec::new()
    }
}

fn dismiss(owner: &str) -> Value {
    json!({"type": "notch.dismiss", "owner": owner})
}

#[cfg(test)]
mod tests {
    use super::*;

    const NANO: Requester = Requester::Module {
        id: "nano",
        display_name: "nano",
    };
    const SNAP: Requester = Requester::Module {
        id: "snap",
        display_name: "snap",
    };

    fn status(t: &str) -> NotchRequest {
        NotchRequest::Status {
            text: t.into(),
            error: false,
        }
    }

    fn validate(lines: &[Value]) {
        let protocol = qrtz_helper_bridge::protocol::PROTOCOL_JSON;
        let raw: Value = serde_json::from_str(protocol).unwrap();
        let types: Vec<&str> = raw["hostToHelperTypes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for l in lines {
            assert!(types.contains(&l["type"].as_str().unwrap()), "{l}");
        }
    }

    #[test]
    fn priority_preempts_and_lower_is_refused() {
        let mut b = NotchBroker::new();
        let out = b.show(&NANO, &status("syncing"), 0, false).unwrap();
        assert_eq!(out[0]["owner"], "module.nano");
        assert_eq!(out[0]["priority"], "module");
        assert_eq!(out[0]["source"], "nano", "attributed with the registered name");
        validate(&out);
        // Another module can't take it.
        assert_eq!(
            b.show(&SNAP, &status("x"), 1, false),
            Err(Refusal::Busy {
                holder: "module.nano".into()
            })
        );
        // Ctrl preempts: the module is dismissed first.
        let out = b
            .show(&Requester::Ctrl, &status("ctrl: opening settings"), 2, false)
            .unwrap();
        assert_eq!(out[0], json!({"type": "notch.dismiss", "owner": "module.nano"}));
        assert_eq!(out[1]["owner"], "ctrl.run");
        assert!(out[1].get("source").is_none());
        validate(&out);
        // And a module is refused, with a reason, while ctrl holds.
        let r = b.show(&NANO, &status("again"), 3, false).unwrap_err();
        assert!(r.to_string().contains("ctrl.run"));
        // Dictation is above everything.
        b.dismiss(&Requester::Ctrl).unwrap();
        b.show(&NANO, &status("back"), 4, false).unwrap();
        assert_eq!(
            b.set_dictation(true),
            vec![json!({"type": "notch.dismiss", "owner": "module.nano"})]
        );
        assert!(matches!(
            b.show(&Requester::Ctrl, &status("ctrl: x"), 5, false),
            Err(Refusal::Busy { .. })
        ));
        assert_eq!(b.holder(), Some("echo.dictation"));
        b.set_dictation(false);
        assert!(b.show(&NANO, &status("ok"), 6, false).is_ok());
    }

    #[test]
    fn only_the_owner_updates_or_dismisses() {
        let mut b = NotchBroker::new();
        assert_eq!(b.update(&NANO, &status("x"), 0, false), Err(Refusal::NotOwner));
        b.show(&NANO, &status("one"), 0, false).unwrap();
        assert_eq!(b.update(&SNAP, &status("x"), 1, false), Err(Refusal::NotOwner));
        assert_eq!(b.dismiss(&SNAP), Err(Refusal::NotOwner));
        assert_eq!(b.dismiss(&Requester::Ctrl), Err(Refusal::NotOwner));
        let up = b.update(&NANO, &status("two"), 2, false).unwrap();
        assert_eq!(up[0]["type"], "notch.update");
        validate(&up);
        assert_eq!(
            b.dismiss(&NANO).unwrap(),
            vec![json!({"type": "notch.dismiss", "owner": "module.nano"})]
        );
    }

    #[test]
    fn module_text_is_sanitized_and_capped() {
        let t = "a\u{202E}b\u{2066}c\nd\u{0007}e\u{200F}  f";
        assert_eq!(sanitize(t), "abc de f");
        let c = content(&status(&"x".repeat(500)), true).unwrap();
        assert_eq!(c["text"].as_str().unwrap().encode_utf16().count(), STATUS_MAX);
        assert!(c["text"].as_str().unwrap().ends_with('…'));
        let l = content(
            &NotchRequest::Icon {
                icon: "check".into(),
                label: "😀".repeat(100),
            },
            true,
        )
        .unwrap();
        assert!(l["label"].as_str().unwrap().encode_utf16().count() <= LABEL_MAX);
        assert_eq!(
            content(&status("\u{202E}\n "), true),
            Err(Refusal::BadContent("empty text"))
        );
        assert!(matches!(
            content(&status("see https://evil.example"), true),
            Err(Refusal::BadContent(_))
        ));
        assert!(matches!(
            content(
                &NotchRequest::Icon {
                    icon: "skull".into(),
                    label: "x".into()
                },
                true
            ),
            Err(Refusal::BadContent(_))
        ));
        assert!(matches!(
            content(&NotchRequest::Level { busy: false }, true),
            Err(Refusal::BadContent(_))
        ));
        assert!(matches!(
            content(
                &NotchRequest::Progress {
                    label: "x".into(),
                    value: Some(f64::NAN)
                },
                true
            ),
            Err(Refusal::BadContent(_))
        ));
        // Unknown kinds (a clickable action) don't even parse.
        assert!(serde_json::from_value::<NotchRequest>(json!({"kind": "action", "label": "pay"})).is_err());
        assert!(serde_json::from_value::<NotchRequest>(
            json!({"kind": "status", "text": "x", "owner": "echo.dictation"})
        )
        .is_err());
        // The display name is sanitized and capped too; owners are wire-safe.
        let mut b = NotchBroker::new();
        let evil = Requester::Module {
            id: "Evil Mod!",
            display_name: "\u{202E}x".repeat(60).leak(),
        };
        let out = b.show(&evil, &status("hi"), 0, false).unwrap();
        assert_eq!(out[0]["owner"], "module.evil-mod-");
        assert!(out[0]["source"].as_str().unwrap().encode_utf16().count() <= SOURCE_MAX);
    }

    #[test]
    fn reserved_prefixes_are_refused_for_modules_only() {
        for t in [
            "qrtz: update ready",
            "Ctrl: deleting files",
            "echo listening",
            "  System alert",
            "macOS needs your password",
        ] {
            assert_eq!(content(&status(t), true), Err(Refusal::ReservedPrefix), "{t}");
        }
        assert!(
            content(&status("\u{202E}qrtz"), true).is_err(),
            "no bidi trick around the prefix rule"
        );
        assert!(
            content(&status("ctrl: opening settings"), false).is_ok(),
            "ctrl's own status may say ctrl:"
        );
        assert!(content(&status("3 events today"), true).is_ok());
    }

    #[test]
    fn module_updates_are_rate_limited() {
        let mut b = NotchBroker::new();
        b.show(&NANO, &status("0"), 0, false).unwrap();
        let mut ok = 1;
        for i in 1..30 {
            if b.update(&NANO, &status(&i.to_string()), 10 * i, false).is_ok() {
                ok += 1;
            }
        }
        assert_eq!(
            ok, MODULE_RATE_PER_SEC,
            "10 in the first second, the rest dropped"
        );
        assert_eq!(
            b.update(&NANO, &status("x"), 500, false),
            Err(Refusal::RateLimited)
        );
        assert!(b.update(&NANO, &status("later"), 1_200, false).is_ok());
        // Ctrl isn't rate-limited.
        let mut c = NotchBroker::new();
        c.show(&Requester::Ctrl, &status("ctrl: a"), 0, false).unwrap();
        for i in 0..50 {
            assert!(c.update(&Requester::Ctrl, &status("ctrl: b"), i, false).is_ok());
        }
    }

    #[test]
    fn module_content_times_out_and_yields_to_secure_input() {
        let mut b = NotchBroker::new();
        b.show(&NANO, &status("x"), 0, false).unwrap();
        assert!(b.tick(9_999, false).is_empty());
        b.update(&NANO, &status("y"), 5_000, false).unwrap();
        assert!(b.tick(14_999, false).is_empty(), "an update restarts the 10 s");
        assert_eq!(
            b.tick(15_000, false),
            vec![json!({"type": "notch.dismiss", "owner": "module.nano"})]
        );
        assert_eq!(b.holder(), None);
        // Secure input: refused, and a shown module is dismissed at the next tick.
        assert_eq!(
            b.show(&NANO, &status("x"), 20_000, true),
            Err(Refusal::SecureInput)
        );
        b.show(&NANO, &status("x"), 20_000, false).unwrap();
        assert_eq!(
            b.update(&NANO, &status("y"), 20_001, true),
            Err(Refusal::SecureInput)
        );
        assert_eq!(b.tick(20_002, true).len(), 1);
        // Ctrl doesn't time out here (its run owns its lifetime).
        b.show(&Requester::Ctrl, &status("ctrl: x"), 0, false).unwrap();
        assert!(b.tick(60_000, true).is_empty());
    }
}
