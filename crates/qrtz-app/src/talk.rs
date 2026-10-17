//! qrtz talk (crates/qrtz-talk, ADR 0058) behind [`TalkPort`]: the helper sink over the supervisor, the
//! validated event bus, the app secret store, the shell clipboard and the config store. While a dictation is
//! live (any phase but idle/error) it holds the shared [`LiveWork`] counter so background indexing yields (N86).
use crate::live::{LiveLatch, LiveWork};
use crate::ports::TalkPort;
use crate::service::{Events, SharedConfig};
use async_trait::async_trait;
use qrtz_helper_bridge::{encode_host_message, HelperState, HelperStatus, Supervisor};
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_talk::{
    Clipboard, HelperAsrSpeech, HelperSink, Providers, RandomIds, SecretSource, SystemClock, TalkDeps,
    TalkEvents, TalkService, Timeouts,
};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// The [`TalkPort`] adapter.
pub struct Talk(pub TalkService);

#[async_trait]
impl TalkPort for Talk {
    async fn call(&self, channel: &str, payload: Value) -> Result<Value, AppError> {
        self.0.call(channel, payload).await
    }
    fn helper_line(&self, msg: &Value) {
        self.0.helper_line(msg)
    }
    fn helper_status(&self, s: &HelperStatus) {
        self.0.helper_status(
            matches!(s.state, HelperState::Running | HelperState::Starting),
            s.last_error.as_deref().unwrap_or("helper stopped"),
        )
    }
    async fn toggle(&self) -> Result<Value, AppError> {
        let v = self.0.toggle().await;
        serde_json::to_value(v).map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))
    }
    fn config_changed(&self) {
        self.0.configure()
    }
}

/// Talk → helper lines over the supervisor (`false` when it isn't running or there is no helper).
pub struct TalkHelper(pub Option<Supervisor>);
impl HelperSink for TalkHelper {
    fn send(&self, msg: &Value) -> bool {
        let Some(s) = &self.0 else { return false };
        match encode_host_message(msg) {
            Ok(line) => s.try_send(line),
            Err(e) => {
                tracing::error!(err = %e, ty = msg["type"].as_str().unwrap_or("?"), "invalid talk host message");
                false
            }
        }
    }
}

/// `talk:state` / `talk:history-changed` onto the validated bus; holds live work while a dictation runs.
pub struct TalkBus {
    pub events: Events,
    pub live: Arc<LiveWork>,
    latch: LiveLatch,
}
impl TalkBus {
    pub fn new(events: Events, live: Arc<LiveWork>) -> Self {
        Self {
            events,
            live,
            latch: LiveLatch::default(),
        }
    }
}
impl TalkEvents for TalkBus {
    fn emit(&self, event: &str, payload: Value) {
        if event == "talk:state" {
            let busy = !matches!(payload["phase"].as_str(), Some("idle" | "error") | None);
            self.latch.set(&self.live, busy);
        }
        self.events.emit(event, payload);
    }
}

/// The cloud engine key (`asr.elevenlabs`) from the app secret store.
pub struct TalkSecrets(pub Arc<dyn crate::secrets::SecretStore>);
impl SecretSource for TalkSecrets {
    fn get(&self, name: &str) -> Option<String> {
        self.0.get(name)
    }
}

/// The main-process clipboard (works while unfocused), via the shell.
pub struct TalkClipboard(pub Arc<dyn crate::storage_port::StorageShell>);
impl Clipboard for TalkClipboard {
    fn write(&self, text: &str) {
        self.0.write_clipboard(text)
    }
}

pub struct TalkWiring {
    pub helper: Option<Supervisor>,
    pub events: Events,
    pub live: Arc<LiveWork>,
    pub secrets: Arc<dyn crate::secrets::SecretStore>,
    pub shell: Arc<dyn crate::storage_port::StorageShell>,
    pub config: SharedConfig,
    pub store_path: PathBuf,
    /// The on-device engine: the speech module with its audio from the helper; `None` = the helper's
    /// `dictation.asr` lines (no module configured in this build).
    pub local: Option<Arc<dyn qrtz_speech::SpeechEngine>>,
    /// Dev knob `QRTZ_TALK_CLOUD=echo|fail`: the cloud engines are `qrtz_talk::mock::MockCloud` (echo every
    /// stream, or refuse every connection) instead of real WebSockets. `None` in production.
    pub cloud: Option<String>,
}

/// The cloud WebSocket transport: real (tungstenite) unless the `talk_cloud` dev knob picks a mock.
fn cloud_connector(knob: Option<&str>) -> Arc<dyn qrtz_talk::ws::WsConnector> {
    use qrtz_talk::contract::Engine;
    use qrtz_talk::mock::{MockCloud, MockMode};
    match knob {
        Some("echo") => Arc::new(MockCloud::new()),
        Some("fail") => Arc::new(MockCloud::new().with(Engine::Elevenlabs, MockMode::Refuse)),
        _ => Arc::new(qrtz_talk::ws::TungsteniteConnector),
    }
}

/// Build the service. The on-device engine is today's helper's `dictation.asr` lines until the qrtz speech
/// module engine is wired (ADR 0046); talk never sees a model name either way.
pub fn talk_service(w: TalkWiring) -> TalkService {
    let asr = Arc::new(HelperAsrSpeech::new());
    let config = w.config;
    TalkService::new(TalkDeps {
        helper: Arc::new(TalkHelper(w.helper)),
        events: Arc::new(TalkBus::new(w.events, w.live)),
        secrets: Arc::new(TalkSecrets(w.secrets)),
        clipboard: Arc::new(TalkClipboard(w.shell)),
        clock: Arc::new(SystemClock::default()),
        ids: Arc::new(RandomIds),
        // No LLM cleanup in talk (Jared, for speed): the text goes in on release, rules only.
        llm: Arc::new(qrtz_talk::NoLlm),
        connector: cloud_connector(w.cloud.as_deref()),
        local: w.local.unwrap_or_else(|| asr.clone()),
        helper_asr: asr,
        config: Arc::new(move || config.get()),
        providers: Providers::default(),
        store_path: w.store_path,
        store_io: Arc::new(qrtz_talk::FsIo),
        timeouts: Timeouts::default(),
    })
}

#[cfg(test)]
mod cloud_knob_tests {
    use super::cloud_connector;
    #[test]
    fn the_talk_cloud_knob_picks_the_connector() {
        // Smoke: every value builds a connector (the mock behaviour itself is qrtz-talk's mock tests).
        for k in [None, Some("echo"), Some("fail"), Some("other")] {
            let _ = cloud_connector(k);
        }
    }
}
