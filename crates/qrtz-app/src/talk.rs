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
    ChatMessage, CleanupLlm, CleanupLlmSource, Clipboard, HelperAsrSpeech, HelperSink, Providers, RandomIds,
    SecretSource, SystemClock, TalkDeps, TalkEvents, TalkService, Timeouts,
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

/// Engine keys (`asr.elevenlabs`, `asr.deepgram`) from the app secret store.
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

/// AI cleanup for dictations: OpenRouter (Claude Haiku 5.5 unless `talk.cleanupModel` names another model) with
/// the `llm.openrouter` key, through `qrtz_llm::http_client()` (no redirects, N81). Local-only never builds it
/// (the talk service also skips the call), and neither does a build without a key: rules cleanup then.
pub struct TalkLlm {
    pub secrets: Arc<dyn crate::secrets::SecretStore>,
    pub config: SharedConfig,
    /// Dev knob `QRTZ_LLM_BASE_URL` (tests and mock servers only, N87).
    pub base_url: Option<String>,
    /// Dev knob `QRTZ_TALK_LLM=fake|hang`: the E2E stand-in (Electron `fakeCleanupLlm`).
    pub fake: Option<String>,
    client: reqwest::Client,
}

impl TalkLlm {
    pub fn new(
        secrets: Arc<dyn crate::secrets::SecretStore>,
        config: SharedConfig,
        base_url: Option<String>,
        fake: Option<String>,
    ) -> Self {
        Self {
            secrets,
            config,
            base_url,
            fake,
            client: qrtz_llm::http_client(),
        }
    }
}

struct ProviderCleanup(Box<dyn qrtz_llm::LlmProvider>);
#[async_trait]
impl CleanupLlm for ProviderCleanup {
    async fn complete(&self, messages: Vec<ChatMessage>) -> Result<String, String> {
        use qrtz_llm::prompt::{ChatMessage as M, Role};
        let msgs: Vec<M> = messages
            .into_iter()
            .map(|m| M {
                role: match m.role {
                    "system" => Role::System,
                    "assistant" => Role::Assistant,
                    _ => Role::User,
                },
                content: m.content,
            })
            .collect();
        self.0.complete(&msgs, false).await.map_err(|e| e.message)
    }
}

/// `fake`: the rules cleanup of the dictation minus a leading "so"; `hang`: never answers (the timeout path).
struct FakeCleanup(bool);
#[async_trait]
impl CleanupLlm for FakeCleanup {
    async fn complete(&self, messages: Vec<ChatMessage>) -> Result<String, String> {
        if self.0 {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let raw = messages.last().map(|m| m.content.as_str()).unwrap_or("");
        let out = qrtz_talk::cleanup::rule_cleanup(raw, &[]);
        let out = out
            .strip_prefix("So, ")
            .or_else(|| out.strip_prefix("So "))
            .unwrap_or(&out);
        Ok(out.to_string())
    }
}

#[async_trait]
impl CleanupLlmSource for TalkLlm {
    async fn llm(&self) -> Option<Arc<dyn CleanupLlm>> {
        if let Some(mode) = &self.fake {
            return Some(Arc::new(FakeCleanup(mode == "hang")));
        }
        let cfg = self.config.get();
        let c = qrtz_llm::LlmConfig {
            provider: "openrouter".into(),
            model: cfg["talk"]["cleanupModel"].as_str().unwrap_or("").to_string(),
            base_url: self.base_url.clone(),
            local_only: cfg["privacy"]["localOnly"].as_bool().unwrap_or(false),
        };
        if c.effective_provider() == "none" {
            return None;
        }
        let key = self.secrets.get("llm.openrouter").filter(|k| !k.is_empty());
        let p = qrtz_llm::create_provider(
            &self.client,
            "openrouter",
            key.map(qrtz_llm::Secret::new),
            Some(&c.model),
            c.base_url.as_deref(),
        )?;
        Some(Arc::new(ProviderCleanup(p)))
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
    /// The cleanup LLM (production: [`TalkLlm`]).
    pub llm: Arc<dyn CleanupLlmSource>,
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
        llm: w.llm,
        connector: Arc::new(qrtz_talk::ws::TungsteniteConnector),
        local: w.local.unwrap_or_else(|| asr.clone()),
        helper_asr: asr,
        config: Arc::new(move || config.get()),
        providers: Providers::default(),
        store_path: w.store_path,
        store_io: Arc::new(qrtz_talk::FsIo),
        timeouts: Timeouts::default(),
    })
}
