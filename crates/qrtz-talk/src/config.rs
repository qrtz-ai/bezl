//! The `talk` part of the app config (zod defaults in packages/core/src/config.ts),
//! read from the full config JSON on every use, like `deps.config()` in TS.
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct TalkConfig {
    pub enabled: bool,
    pub push_key: String,
    /// `talk.blockGlobeKey`: with `fn` as the push key, the helper swallows the Fn/globe events (`consumeFn`).
    pub block_globe_key: bool,
    pub warm_mic: bool,
    pub pre_roll_ms: i64,
    /// `talk.autoCleanup`: the on-device rules. There is no LLM cleanup (ADR 0071); an old config's `aiCleanup`,
    /// `cleanupModel` and `cleanupTimeoutMs` are ignored here (qrtz-app's loader drops them).
    pub auto_cleanup: bool,
    pub auto_insert: bool,
    pub restore_clipboard: bool,
    pub save_history: bool,
    /// `talk.showPill`: the helper's native pill (ADR 0092, `dictation.configure{pill}`).
    pub show_pill: bool,
    /// `talk.echoGate` (ADR 0097, N153): while dictating with the speakers playing, gate the mic against the
    /// system audio (a short-lived system tap under the screen & system audio grant). `dictation.configure{echoGate}`.
    pub echo_gate: bool,
}

impl TalkConfig {
    pub fn from_app_config(cfg: &Value) -> Self {
        let t = &cfg["talk"];
        let b = |k: &str, d: bool| t[k].as_bool().unwrap_or(d);
        Self {
            enabled: b("enabled", true),
            push_key: t["pushKey"].as_str().unwrap_or("fn").to_owned(),
            block_globe_key: b("blockGlobeKey", true),
            warm_mic: b("warmMic", false),
            pre_roll_ms: t["preRollMs"].as_i64().unwrap_or(300).clamp(0, 2000),
            auto_cleanup: b("autoCleanup", true),
            auto_insert: b("autoInsert", true),
            restore_clipboard: b("restoreClipboard", true),
            save_history: b("saveHistory", true),
            show_pill: b("showPill", true),
            echo_gate: b("echoGate", true),
        }
    }
}
