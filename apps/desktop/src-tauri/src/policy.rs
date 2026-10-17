//! Pure shell policy, mirroring apps/desktop/src/main/index.ts so both shells behave the same.
use serde_json::Value;
use std::time::Duration;

pub const RECORD_TOGGLE_KEY: &str = "CommandOrControl+Shift+R";

/// HUD is up while a session is live or finishing, if Settings → floating hud is on.
pub fn hud_wanted(config: &Value, session: &Value) -> bool {
    config["note"]["floatingHud"].as_bool().unwrap_or(true)
        && matches!(
            session["phase"].as_str().unwrap_or("idle"),
            "starting" | "recording" | "reconnecting" | "stopping" | "processing"
        )
}
/// Linger so "summarizing…" → done is visible, then close.
pub const HUD_LINGER: Duration = Duration::from_millis(1500);

pub fn pill_wanted(config: &Value, talk: &Value) -> bool {
    config["talk"]["showPill"].as_bool().unwrap_or(true) && talk["phase"].as_str().unwrap_or("idle") != "idle"
}
pub fn pill_linger(talk: &Value) -> Duration {
    Duration::from_millis(if talk["phase"].as_str() == Some("error") {
        2500
    } else {
        400
    })
}

/// Global shortcuts the config asks for: (accelerator, action). Empty strings mean unbound.
pub fn hotkeys(config: &Value) -> Vec<(String, &'static str)> {
    let mut out = vec![(RECORD_TOGGLE_KEY.to_string(), "record")];
    let pause = config["privacy"]["pauseHotkey"].as_str().unwrap_or("");
    if !pause.is_empty() {
        out.push((pause.to_string(), "pause"));
    }
    if config["talk"]["enabled"].as_bool().unwrap_or(false) {
        let talk = config["talk"]["toggleHotkey"].as_str().unwrap_or("");
        if !talk.is_empty() {
            out.push((talk.to_string(), "talk"));
        }
    }
    out.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hud_follows_phase_and_setting() {
        let on = json!({"note": {"floatingHud": true}});
        let off = json!({"note": {"floatingHud": false}});
        for p in ["starting", "recording", "reconnecting", "stopping", "processing"] {
            assert!(hud_wanted(&on, &json!({ "phase": p })), "{p}");
            assert!(!hud_wanted(&off, &json!({ "phase": p })), "{p}");
        }
        for p in ["idle", "detected", "done", "error"] {
            assert!(!hud_wanted(&on, &json!({ "phase": p })), "{p}");
        }
    }

    #[test]
    fn pill_follows_talk_phase() {
        let c = json!({"talk": {"showPill": true}});
        assert!(pill_wanted(&c, &json!({"phase": "listening"})));
        assert!(!pill_wanted(&c, &json!({"phase": "idle"})));
        assert!(!pill_wanted(
            &json!({"talk": {"showPill": false}}),
            &json!({"phase": "listening"})
        ));
        assert_eq!(
            pill_linger(&json!({"phase": "error"})),
            Duration::from_millis(2500)
        );
        assert_eq!(pill_linger(&json!({"phase": "idle"})), Duration::from_millis(400));
    }

    #[test]
    fn hotkeys_from_config() {
        let c = json!({"privacy": {"pauseHotkey": "Alt+P"}, "talk": {"enabled": true, "toggleHotkey": "Alt+Space"}});
        assert_eq!(
            hotkeys(&c),
            vec![
                ("CommandOrControl+Shift+R".to_string(), "record"),
                ("Alt+P".to_string(), "pause"),
                ("Alt+Space".to_string(), "talk")
            ]
        );
        let c =
            json!({"privacy": {"pauseHotkey": ""}, "talk": {"enabled": false, "toggleHotkey": "Alt+Space"}});
        assert_eq!(hotkeys(&c).len(), 1);
    }
}
