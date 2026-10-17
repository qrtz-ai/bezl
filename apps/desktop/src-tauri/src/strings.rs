//! Every user-visible string the Rust shell renders (window titles, tray, menus). Lowercase by brand rule;
//! scripts/check-titles.mjs scans `UI_*` consts and `.title(…)`/menu literals in this crate.
pub const UI_APP: &str = "qrtz";
pub const UI_HUD: &str = "qrtz hud";
pub const UI_PILL: &str = "qrtz echo";
pub const UI_TALK_FAILED: &str = "dictation didn't go through";
pub const UI_OPEN: &str = "open qrtz";
pub const UI_QUIT: &str = "quit qrtz";
pub const UI_START: &str = "start recording";
pub const UI_RECORD_DETECTED: &str = "record this meeting";
pub const UI_STOP: &str = "stop recording";
pub const UI_NOT_NOW: &str = "not now";
pub const UI_NOT_RECORDING: &str = "not recording";
pub const UI_SUMMARIZING: &str = "summarizing…";
pub const UI_HELPER_MISSING: &str = "recording unavailable (helper missing)";
pub const UI_MEETING_DETECTED: &str = "meeting detected";
pub const UI_RECORDING: &str = "recording";
pub const UI_NO_AUDIO_TITLE: &str = "no audio is being recorded";
pub const UI_NO_AUDIO_BODY: &str = "check your mic and meeting audio";
