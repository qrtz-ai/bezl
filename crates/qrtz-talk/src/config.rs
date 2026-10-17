//! The `talk` and `privacy.localOnly` parts of the app config (zod defaults in packages/core/src/config.ts),
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
    /// `on-device` | `auto` (= on-device here: the cloud is opt-in) | `elevenlabs`. A legacy `deepgram` (removed engine) is migrated to `auto` by qrtz-app's config loader, and
    /// anything else unknown is on-device too.
    pub engine: String,
    pub auto_failover: bool,
    pub auto_cleanup: bool,
    pub ai_cleanup: bool,
    pub cleanup_timeout_ms: u64,
    pub auto_insert: bool,
    pub restore_clipboard: bool,
    pub save_history: bool,
    pub local_only: bool,
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
            engine: t["engine"].as_str().unwrap_or("auto").to_owned(),
            auto_failover: b("autoFailover", true),
            auto_cleanup: b("autoCleanup", true),
            ai_cleanup: b("aiCleanup", true),
            cleanup_timeout_ms: t["cleanupTimeoutMs"].as_u64().unwrap_or(900).clamp(100, 10_000),
            auto_insert: b("autoInsert", true),
            restore_clipboard: b("restoreClipboard", true),
            save_history: b("saveHistory", true),
            local_only: cfg["privacy"]["localOnly"].as_bool().unwrap_or(false),
        }
    }
}
