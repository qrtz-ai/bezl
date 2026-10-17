//! The talk service: push-to-talk dictation. idle → listening → finalizing → cleaning → inserting → idle (or
//! error). The helper opens the mic; the on-device engine (`qrtz_speech::SpeechEngine`, the qrtz speech module) is
//! the default, so dictation works offline. An opt-in cloud provider (ElevenLabs) runs alongside when the user picks it, and
//! the [`Arbiter`] fails over to the on-device text when it breaks; with auto-failover off an unreachable cloud
//! ends the dictation at once with `errorKind: offline`. Every wait has a timeout so a dictation always ends.
//!
//! Port of `apps/desktop/src/main/talk-service.ts` (busy ids 8eddeab, T1 2ea8a53, H5, T3/T4); ADR 0058.
use crate::arbiter::Arbiter;
use crate::cleanup::{cleanup_dictation, CleanupLlm};
use crate::config::TalkConfig;
use crate::contract::*;
use crate::provider::{ElevenLabsProvider, StreamingProvider};
use crate::speech::{
    bound_final, cloud_fail_text, drive, fail_error, max_rejects, start_error, CloudSpeech, HelperAsrSpeech,
    SessionCtl, TalkValidator,
};
use crate::store::{self, LoadOutcome, Store, StoreIo, MAX_HISTORY};
use crate::ws::WsConnector;
use async_trait::async_trait;
use base64::Engine as _;
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_speech::engine::StatusPhase;
use qrtz_speech::{Channel, FailReason, SessionKind, SessionSpec, SpeakerMode, SpeechEngine, SpeechEvent};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

// ------------------------------------------------------------------------------------------------- injected deps

/// Host → helper lines (qrtz-app: encode + `Supervisor::try_send`). `false` = the helper isn't running.
pub trait HelperSink: Send + Sync {
    fn send(&self, msg: &Value) -> bool;
}
/// `talk:state` / `talk:history-changed` onto the validated IPC bus.
pub trait TalkEvents: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}
/// API keys from qrtz-app's secret store. Values are only ever handed to a provider's request.
pub trait SecretSource: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}
pub trait Clipboard: Send + Sync {
    fn write(&self, text: &str);
}
pub trait Clock: Send + Sync {
    /// Wall clock, epoch ms (`at`, `startedAt`, file names).
    fn now_ms(&self) -> f64;
    /// Monotonic ms (latencies, durations, stall and timeout checks; immune to clock jumps).
    fn mono_ms(&self) -> f64;
}
pub trait IdSource: Send + Sync {
    fn next_id(&self) -> String;
}
/// Builds the cleanup LLM call for this dictation (`None` = none configured; rules only).
#[async_trait]
pub trait CleanupLlmSource: Send + Sync {
    async fn llm(&self) -> Option<Arc<dyn CleanupLlm>>;
}

pub struct SystemClock {
    origin: std::time::Instant,
}
impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}
impl Clock for SystemClock {
    fn now_ms(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    }
    fn mono_ms(&self) -> f64 {
        self.origin.elapsed().as_millis() as f64
    }
}

/// Random v4-style ids from the OS RNG via `std` hashing (no extra dependency; uniqueness, not secrecy).
pub struct RandomIds;
impl IdSource for RandomIds {
    fn next_id(&self) -> String {
        use std::hash::{BuildHasher, Hasher};
        let mut b = [0u8; 16];
        for chunk in b.chunks_mut(8) {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            );
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }
}

pub struct NoLlm;
#[async_trait]
impl CleanupLlmSource for NoLlm {
    async fn llm(&self) -> Option<Arc<dyn CleanupLlm>> {
        None
    }
}

/// The cloud providers a user can opt into (`talk.engine`).
pub struct Providers {
    pub elevenlabs: Arc<dyn StreamingProvider>,
}
impl Default for Providers {
    fn default() -> Self {
        Self {
            elevenlabs: Arc::new(ElevenLabsProvider::default()),
        }
    }
}

/// Timeouts (ms), the TS defaults; tests shrink them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeouts {
    /// `dictation.start` → `dictation.started`.
    pub started: u64,
    /// `dictation.stop` → `dictation.stopped`.
    pub stopped: u64,
    /// `insert.text` → `insert.result`.
    pub insert: u64,
    /// Cloud silent this long while on-device already has words → failed over (auto-failover on).
    pub stall: u64,
    /// Cloud flush → final text.
    pub final_ms: u64,
    /// Stall check period.
    pub check: u64,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            started: 3000,
            stopped: 3000,
            insert: 2500,
            stall: 1500,
            final_ms: 1500,
            check: 250,
        }
    }
}

pub struct TalkDeps {
    pub helper: Arc<dyn HelperSink>,
    pub events: Arc<dyn TalkEvents>,
    pub secrets: Arc<dyn SecretSource>,
    pub clipboard: Arc<dyn Clipboard>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    pub llm: Arc<dyn CleanupLlmSource>,
    pub connector: Arc<dyn WsConnector>,
    /// The on-device engine (`qrtz_speech::SpeechEngine`: the qrtz speech module). Talk never knows the model.
    pub local: Arc<dyn SpeechEngine>,
    /// Today's helper's `dictation.asr` lines as a `SpeechEngine` (transitional; pass the same instance as
    /// `local` until the module ships, or any instance once `local` is the module).
    pub helper_asr: Arc<HelperAsrSpeech>,
    /// The full app config JSON (read on every use).
    pub config: Arc<dyn Fn() -> Value + Send + Sync>,
    pub providers: Providers,
    /// `<data dir>/talk.json` (the same file Electron used).
    pub store_path: PathBuf,
    pub store_io: Arc<dyn StoreIo>,
    pub timeouts: Timeouts,
}

// ------------------------------------------------------------------------------------------------- state

struct Attempt {
    id: String,
    arbiter: Arbiter,
    /// The opt-in cloud provider and its key (`None` = on-device only).
    cloud: Option<(Arc<dyn StreamingProvider>, Option<String>)>,
    session: Option<SessionCtl>,
    /// Cloud audio that arrived before its session was up (flushed on attach).
    pending_audio: Vec<Vec<u8>>,
    cloud_val: TalkValidator,
    /// The on-device session (pinned by the engine to the module version it started on); `None` once it failed.
    local: Option<SessionCtl>,
    local_val: TalkValidator,
    local_done: Option<oneshot::Sender<()>>,
    /// An on-device session is running or starting (the fallback exists).
    local_wanted: bool,
    cloud_finals: Vec<String>,
    cloud_partial: String,
    gen: u64,
    auto_failover: bool,
    final_tx: Option<oneshot::Sender<Result<String, String>>>,
}

struct Inner {
    view: TalkView,
    store: Store,
    hellos: u64,
    dictation_helper: u64,
    via_push_key: bool,
    key_down: f64,
    key_up: f64,
    first_partial: Option<f64>,
    waiters: HashMap<String, oneshot::Sender<Option<Value>>>,
    attempt: Option<Attempt>,
}

struct Shared {
    deps: TalkDeps,
    inner: Mutex<Inner>,
}

/// The service. Cheap to clone (one shared state); every method may be called from any tokio task.
#[derive(Clone)]
pub struct TalkService(Arc<Shared>);

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}

const MSG_OFFLINE: &str =
    "the cloud speech provider can't be reached and automatic failover is off, so dictation stopped";
const MSG_BUSY: &str = "another dictation is still running";

impl TalkService {
    pub fn new(deps: TalkDeps) -> Self {
        let (st, outcome) = store::load(deps.store_io.as_ref(), &deps.store_path, deps.clock.now_ms());
        match &outcome {
            LoadOutcome::Corrupt { moved_to } => {
                tracing::warn!(
                    moved = moved_to.is_some(),
                    "talk store was not valid JSON; moved aside, starting empty"
                )
            }
            LoadOutcome::Unreadable(kind) => tracing::warn!(%kind, "talk store unreadable; starting empty"),
            LoadOutcome::Loaded { dropped } if *dropped > 0 => {
                tracing::warn!(dropped, "talk store: invalid entries dropped")
            }
            _ => {}
        }
        Self(Arc::new(Shared {
            deps,
            inner: Mutex::new(Inner {
                view: TalkView::idle(),
                store: st,
                hellos: 0,
                dictation_helper: 0,
                via_push_key: false,
                key_down: 0.0,
                key_up: 0.0,
                first_partial: None,
                waiters: HashMap::new(),
                attempt: None,
            }),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn deps(&self) -> &TalkDeps {
        &self.0.deps
    }
    fn cfg(&self) -> TalkConfig {
        TalkConfig::from_app_config(&(self.deps().config)())
    }
    fn mono(&self) -> f64 {
        self.deps().clock.mono_ms()
    }
    fn send(&self, msg: Value) -> bool {
        self.deps().helper.send(&msg)
    }
    fn emit_view(&self, g: &Inner) {
        self.deps()
            .events
            .emit("talk:state", serde_json::to_value(&g.view).unwrap_or(Value::Null));
    }
    fn save(&self, g: &Inner) {
        if let Err(e) = self
            .deps()
            .store_io
            .write_private_atomic(&self.deps().store_path, &store::serialize(&g.store))
        {
            // Never the content: only the error kind. The in-memory state is kept and the next write carries it.
            tracing::warn!(kind = %e.kind(), "talk store write failed");
        }
    }

    // --------------------------------------------------------------------------------------------- reads

    pub fn state(&self) -> TalkView {
        self.lock().view.clone()
    }
    pub fn history(&self) -> Vec<TalkEntry> {
        self.lock().store.history.clone()
    }
    pub fn dictionary(&self) -> Vec<DictionaryEntry> {
        self.lock().store.dictionary.clone()
    }
    pub fn set_dictionary(&self, entries: Vec<DictionaryEntry>) -> Vec<DictionaryEntry> {
        let mut g = self.lock();
        let mut seen = std::collections::HashSet::new();
        g.store.dictionary = entries
            .into_iter()
            .filter_map(|e| e.normalized())
            .filter(|e| seen.insert(e.term.to_lowercase()))
            .collect();
        self.save(&g);
        g.store.dictionary.clone()
    }
    pub fn delete_entry(&self, id: &str) -> bool {
        let mut g = self.lock();
        let n = g.store.history.len();
        g.store.history.retain(|h| h.id != id);
        self.save(&g);
        self.deps().events.emit("talk:history-changed", json!({"id": id}));
        g.store.history.len() != n
    }
    pub fn clear_history(&self) {
        let mut g = self.lock();
        g.store.history.clear();
        self.save(&g);
        self.deps()
            .events
            .emit("talk:history-changed", json!({"id": "*"}));
    }
    pub fn copy(&self, id: &str) -> bool {
        let g = self.lock();
        match g.store.history.iter().find(|h| h.id == id) {
            Some(e) => {
                self.deps().clipboard.write(&e.text);
                true
            }
            None => false,
        }
    }

    /// The `talk:*` channels (payloads already validated by qrtz-ipc against the contract).
    pub async fn call(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        let view = |v: TalkView| Ok(serde_json::to_value(v).unwrap_or(Value::Null));
        match channel {
            "talk:state" => view(self.state()),
            "talk:start" => view(self.start().await),
            "talk:stop" => view(self.stop().await),
            "talk:cancel" => view(self.cancel()),
            "talk:history" => Ok(json!(self.history())),
            "talk:delete-entry" => Ok(json!({"ok": self.delete_entry(&s(&p, "id"))})),
            "talk:clear-history" => {
                self.clear_history();
                Ok(json!({"ok": true}))
            }
            "talk:copy" => Ok(json!({"ok": self.copy(&s(&p, "id"))})),
            "talk:dictionary" => Ok(json!(self.dictionary())),
            "talk:set-dictionary" => {
                let entries: Vec<DictionaryEntry> = serde_json::from_value(p["entries"].clone())
                    .map_err(|e| AppError::new(ErrorCode::Validation, format!("talk:set-dictionary: {e}")))?;
                Ok(json!(self.set_dictionary(entries)))
            }
            _ => Err(AppError::new(
                ErrorCode::UnknownChannel,
                format!("no talk handler for {channel}"),
            )),
        }
    }

    /// Toggle hotkey: listening → stop, otherwise start.
    pub async fn toggle(&self) -> TalkView {
        if self.state().phase == TalkPhase::Listening {
            self.stop().await
        } else {
            self.start().await
        }
    }

    // --------------------------------------------------------------------------------------------- engine

    /// The cloud provider a new dictation streams to, if any. On-device is the default (`auto` and `on-device`);
    /// a cloud engine only when picked explicitly, its key exists and local-only privacy is off. A picked engine
    /// without a key falls back to on-device, like Electron's `resolveEngine`.
    pub fn resolve_cloud(&self, cfg: &TalkConfig) -> Option<(Arc<dyn StreamingProvider>, Option<String>)> {
        if cfg.local_only {
            return None;
        }
        let p = &self.deps().providers;
        let pr = match cfg.engine.as_str() {
            "elevenlabs" => p.elevenlabs.clone(),
            _ => return None,
        };
        let key = pr
            .secret_name()
            .and_then(|n| self.deps().secrets.get(n))
            .filter(|k| !k.is_empty());
        pr.configured(key.as_deref()).then_some((pr, key))
    }

    /// Which engine a new dictation uses.
    pub fn resolve_engine(&self) -> Engine {
        self.resolve_cloud(&self.cfg())
            .map(|(p, _)| p.engine())
            .unwrap_or(Engine::OnDevice)
    }

    /// Push settings to the helper. Call on start, every helper hello and every config or secret change.
    pub fn configure(&self) {
        let cfg = self.cfg();
        let stream = self.resolve_cloud(&cfg).is_some();
        self.send(json!({
            "type": "dictation.configure",
            "warmMic": cfg.enabled && cfg.warm_mic,
            "preRollMs": cfg.pre_roll_ms,
            "streamAudio": stream,
            "pushKey": if cfg.enabled { cfg.push_key.clone() } else { String::new() },
            "consumeFn": cfg.block_globe_key,
        }));
    }

    // --------------------------------------------------------------------------------------------- helper in

    /// A validated helper → host line: `hello`, `hotkey`, `dictation.*`, `insert.*` and `error{dictation.*}`.
    pub fn helper_line(&self, m: &Value) {
        // The speech module's lines (and today's helper's `dictation.asr`) are validated by the engine first.
        if self.deps().helper_asr.on_helper_line(m) {
            return;
        }
        let ty = m["type"].as_str().unwrap_or_default();
        match ty {
            "hello" => {
                // A fresh helper process: a dictation started under the previous one is gone (T1). Lines written
                // before a process's first hello reach that process, so a dictation before any hello is helper 1's.
                let lost = {
                    let mut g = self.lock();
                    g.hellos += 1;
                    g.dictation_helper < g.hellos
                };
                if lost {
                    self.helper_lost("helper restarted");
                }
                self.configure();
            }
            "hotkey" => {
                if !self.cfg().enabled {
                    return;
                }
                let action = s(m, "action");
                let mut g = self.lock();
                if action == "down" && !g.view.active() {
                    g.via_push_key = true;
                    drop(g);
                    let me = self.clone();
                    tokio::spawn(async move { me.start().await });
                } else if action == "up" && g.via_push_key {
                    g.via_push_key = false;
                    drop(g);
                    let me = self.clone();
                    tokio::spawn(async move { me.stop().await });
                }
            }
            "dictation.started" => self.resolve(&format!("started:{}", s(m, "id")), m.clone()),
            "dictation.level" => {
                let mut g = self.lock();
                if g.view.phase == TalkPhase::Listening {
                    g.view.level = m["rms"].as_f64().unwrap_or(0.0);
                    self.emit_view(&g);
                }
            }
            "dictation.audio" => {
                let Ok(pcm) =
                    base64::engine::general_purpose::STANDARD.decode(m["pcm16"].as_str().unwrap_or_default())
                else {
                    return;
                };
                let mut g = self.lock();
                let Some(a) = g.attempt.as_mut().filter(|a| a.id == s(m, "id")) else {
                    return;
                };
                match &a.session {
                    Some(sess) => sess.push_audio(pcm),
                    // The cloud session is still being set up: keep it (bounded: ~10 s at 16 kHz mono).
                    None if a.cloud.is_some() && a.pending_audio.len() < 500 => a.pending_audio.push(pcm),
                    None => {}
                }
            }
            "dictation.stopped" => {
                let id = s(m, "id");
                let key = format!("stopped:{id}");
                let g = self.lock();
                if g.waiters.contains_key(&key) {
                    drop(g);
                    return self.resolve(&key, m.clone());
                }
                // H5: the helper ended the dictation itself. Keep and insert what was heard, then say why.
                let reason = s(m, "reason");
                if !reason.is_empty()
                    && g.view.id.as_deref() == Some(id.as_str())
                    && g.view.phase == TalkPhase::Listening
                {
                    drop(g);
                    let (kind, msg) = if reason == "permission-revoked" {
                        (
                            TalkErrorKind::PermissionRevoked,
                            "microphone access was turned off, so dictation stopped",
                        )
                    } else {
                        (
                            TalkErrorKind::MicLost,
                            "the microphone went away, so dictation stopped",
                        )
                    };
                    let text = bound_final(&s(m, "text"));
                    let dur = m["durationMs"].as_f64().unwrap_or(0.0);
                    let me = self.clone();
                    tokio::spawn(async move { me.ended_by_helper(id, text, dur, kind, msg).await });
                }
            }
            "permission.revoked" => {
                if m["permission"] == "microphone" && self.lock().view.active() {
                    tracing::warn!("microphone revoked mid-dictation");
                }
            }
            "insert.result" => self.resolve(&format!("insert:{}", s(m, "id")), m.clone()),
            "error" => {
                let code = s(m, "code");
                if !code.starts_with("dictation.") {
                    return;
                }
                tracing::warn!(%code, "dictation error");
                if code == "dictation.mic" {
                    let mut g = self.lock();
                    if g.view.active() {
                        let msg = m["message"]
                            .as_str()
                            .unwrap_or("microphone access denied")
                            .to_owned();
                        self.fail(&mut g, TalkErrorKind::MicDenied, &msg, true);
                    }
                } else if code == "dictation.busy" {
                    self.on_busy(m);
                }
            }
            _ => {}
        }
    }

    /// Supervisor status: not running and not starting while a dictation is active → T1 helper-lost.
    pub fn helper_status(&self, running_or_starting: bool, why: &str) {
        if !running_or_starting {
            self.helper_lost(why);
        }
    }

    /// error{dictation.busy, id, activeId}: act only when `id` is our start in flight. `activeId == id` is a
    /// duplicate start of our running dictation (keep it); any other `activeId` is an orphan: cancel it by its
    /// own id, then fail this attempt without cancelling it (the helper never ran it). Never cancels by view id.
    fn on_busy(&self, m: &Value) {
        let (bid, active) = (s(m, "id"), s(m, "activeId"));
        let mut g = self.lock();
        let id = g.view.id.clone().unwrap_or_default();
        if bid.is_empty() || bid != id || g.view.phase != TalkPhase::Listening {
            tracing::warn!(id = %bid, current = %id, "dictation.busy for another start; ignored");
            return;
        }
        if active == id {
            if let Some(tx) = g.waiters.remove(&format!("started:{id}")) {
                let _ = tx.send(Some(
                    json!({"type": "dictation.started", "id": id, "preRollMs": 0}),
                ));
            }
            return;
        }
        if !active.is_empty() {
            tracing::warn!(active_id = %active, "cancelling orphaned dictation");
            self.send(json!({"type": "dictation.cancel", "id": active}));
        }
        self.fail(&mut g, TalkErrorKind::Busy, MSG_BUSY, false);
    }

    /// T1: the helper that served the dictation is gone (supervisor down, or a new hello). End it gracefully with
    /// what was heard, release every waiter for its id. Idempotent; a no-op when idle.
    pub fn helper_lost(&self, why: &str) {
        let mut g = self.lock();
        let Some(id) = g.view.id.clone().filter(|_| g.view.active()) else {
            return;
        };
        tracing::warn!(why, phase = ?g.view.phase, "helper lost mid-dictation");
        g.via_push_key = false;
        for k in ["started", "stopped", "insert"] {
            if let Some(tx) = g.waiters.remove(&format!("{k}:{id}")) {
                let _ = tx.send(None);
            }
        }
        if g.view.phase == TalkPhase::Listening {
            let dur = self.mono() - g.key_down;
            drop(g);
            let me = self.clone();
            tokio::spawn(async move {
                me.ended_by_helper(
                    id,
                    String::new(),
                    dur,
                    TalkErrorKind::HelperLost,
                    "the capture helper restarted, so dictation stopped",
                )
                .await
            });
        }
    }

    fn resolve(&self, key: &str, m: Value) {
        if let Some(tx) = self.lock().waiters.remove(key) {
            let _ = tx.send(Some(m));
        }
    }

    fn waiter(g: &mut Inner, key: String) -> oneshot::Receiver<Option<Value>> {
        let (tx, rx) = oneshot::channel();
        g.waiters.insert(key, tx);
        rx
    }

    async fn wait(&self, key: String, rx: oneshot::Receiver<Option<Value>>, ms: u64) -> Option<Value> {
        let r = tokio::time::timeout(Duration::from_millis(ms), rx).await;
        self.lock().waiters.remove(&key);
        r.ok().and_then(|r| r.ok()).flatten()
    }

    fn refresh(&self, g: &mut Inner) {
        let now = self.mono();
        let Some(a) = g.attempt.as_ref() else { return };
        let (c, p) = a.arbiter.view();
        let (engine, failover) = (a.arbiter.engine(), a.arbiter.failover());
        if (!c.is_empty() || !p.is_empty()) && g.first_partial.is_none() {
            g.first_partial = Some(now);
        }
        g.view.text = c;
        g.view.partial = p;
        g.view.engine = Some(engine);
        g.view.failover = Some(failover);
        self.emit_view(g);
    }

    // --------------------------------------------------------------------------------------------- cloud

    fn on_event(&self, id: &str, gen: u64, local: bool) -> Arc<dyn Fn(SpeechEvent) + Send + Sync> {
        let me = Arc::downgrade(&self.0);
        let id = id.to_owned();
        Arc::new(move |ev| {
            if let Some(sh) = me.upgrade() {
                let svc = TalkService(sh);
                if local {
                    svc.on_local(&id, ev)
                } else {
                    svc.on_cloud(&id, gen, ev)
                }
            }
        })
    }

    fn spec(id: &str) -> SessionSpec {
        SessionSpec {
            session_id: id.to_owned(),
            kind: SessionKind::Dictation,
            channels: vec![Channel::Mic],
            language_hint: None,
            speaker_mode: SpeakerMode::Fast,
        }
    }

    /// The opt-in cloud engine, as one more `SpeechEngine` in the host (called under the state lock: the start
    /// itself runs on a task; audio until then is kept in `pending_audio`).
    fn connect_cloud(&self, a: &mut Attempt, dictionary: Vec<DictionaryEntry>) {
        let Some((pr, key)) = a.cloud.clone() else { return };
        a.gen += 1;
        a.arbiter.attach_cloud(pr.engine());
        let engine = CloudSpeech {
            provider: pr,
            key,
            connector: self.deps().connector.clone(),
            dictionary,
        };
        let (me, id, gen) = (self.clone(), a.id.clone(), a.gen);
        let on = self.on_event(&id, gen, false);
        tokio::spawn(async move {
            match engine.start(Self::spec(&id)).await {
                Ok(sess) => {
                    let ctl = drive(sess, Channel::Mic, on);
                    let mut g = me.lock();
                    if let Some(a) = g
                        .attempt
                        .as_mut()
                        .filter(|a| a.id == id && a.gen == gen && a.arbiter.using_cloud())
                    {
                        for pcm in a.pending_audio.drain(..) {
                            ctl.push_audio(pcm);
                        }
                        a.session = Some(ctl);
                    }
                }
                Err(e) => {
                    let mut g = me.lock();
                    if g.attempt.as_ref().is_some_and(|a| a.id == id && a.gen == gen) {
                        me.cloud_failed(
                            &mut g,
                            Some(format!("cloud engine refused: {}", start_error(&e).1)),
                        );
                    }
                }
            }
        });
    }

    /// Validated on-device words, or the engine going away.
    fn on_local(&self, id: &str, ev: SpeechEvent) {
        let now = self.mono();
        let mut g = self.lock();
        let Some(a) = g.attempt.as_mut().filter(|a| a.id == id) else {
            return;
        };
        let failed = match &ev {
            SpeechEvent::Words { .. } => match a.local_val.words(&ev) {
                Ok(h) => {
                    let stalled = if h.is_final {
                        a.arbiter.local_final(&h.text, now)
                    } else {
                        a.arbiter.local_partial(&h.text, now)
                    };
                    if stalled {
                        self.cloud_failed(&mut g, None);
                    } else {
                        self.refresh(&mut g);
                    }
                    None
                }
                Err(r) => {
                    tracing::warn!(why = r.0, "rejected on-device speech output");
                    (a.local_val.rejected >= max_rejects()).then_some(FailReason::ProtocolViolations)
                }
            },
            SpeechEvent::Finished { .. } => {
                a.local = None;
                if let Some(tx) = a.local_done.take() {
                    let _ = tx.send(());
                }
                None
            }
            SpeechEvent::Status {
                phase: StatusPhase::Failed,
                error,
            } => {
                tracing::warn!(?error, "on-device speech status failed");
                Some(FailReason::StartFailed)
            }
            SpeechEvent::Failed { reason } => Some(*reason),
            // Speaker turns are diarization (note's); talk never names a speaker.
            SpeechEvent::Status { .. } | SpeechEvent::Speaker { .. } => None,
        };
        if let Some(reason) = failed {
            self.local_failed(g, id, reason);
        }
    }

    /// The on-device session is gone mid-dictation. With the cloud carrying the dictation it continues; otherwise
    /// it ends cleanly with what was heard: `speech-unavailable` when the module was revoked, went stale or isn't
    /// installed (the engine ends a revoked session at its next safe point, after the utterance in flight), else
    /// `speech-lost` (crash, protocol violations).
    fn local_failed(&self, mut g: MutexGuard<'_, Inner>, id: &str, reason: FailReason) {
        let now = self.mono();
        let Some(a) = g.attempt.as_mut().filter(|a| a.id == id) else {
            return;
        };
        tracing::warn!(?reason, "on-device speech ended");
        a.local = None;
        a.local_wanted = false;
        if let Some(tx) = a.local_done.take() {
            let _ = tx.send(());
        }
        let heard = a.arbiter.local_live();
        if a.arbiter.using_cloud() || g.view.phase != TalkPhase::Listening {
            return;
        }
        let (id, dur) = (id.to_owned(), now - g.key_down);
        self.send(json!({"type": "dictation.cancel", "id": id}));
        drop(g);
        let me = self.clone();
        tokio::spawn(async move {
            let (kind, msg) = fail_error(reason);
            me.ended_by_helper(id, heard, dur, kind, msg).await
        });
    }

    fn on_cloud(&self, id: &str, gen: u64, ev: SpeechEvent) {
        let mut g = self.lock();
        let Some(a) = g.attempt.as_mut().filter(|a| a.id == id && a.gen == gen) else {
            return;
        };
        match &ev {
            SpeechEvent::Words { .. } => match a.cloud_val.words(&ev) {
                Ok(h) => {
                    if h.is_final {
                        if !h.text.is_empty() {
                            a.cloud_finals.push(h.text);
                        }
                        a.cloud_partial.clear();
                    } else {
                        a.cloud_partial = h.text;
                    }
                    let committed = a.cloud_finals.join(" ");
                    let partial = a.cloud_partial.clone();
                    a.arbiter.cloud_update(&committed, &partial);
                    if matches!(g.view.phase, TalkPhase::Listening | TalkPhase::Finalizing) {
                        self.refresh(&mut g);
                    }
                }
                Err(r) => {
                    tracing::warn!(why = r.0, "rejected cloud speech output");
                    if a.cloud_val.rejected >= max_rejects() {
                        self.cloud_failed(&mut g, Some("invalid provider output".into()));
                    }
                }
            },
            SpeechEvent::Finished { .. } => {
                a.session = None;
                let text = a.cloud_finals.join(" ");
                if let Some(tx) = a.final_tx.take() {
                    let _ = tx.send(Ok(text));
                }
            }
            SpeechEvent::Failed { reason } => {
                let r = cloud_fail_text(*reason).to_owned();
                self.cloud_failed(&mut g, Some(r))
            }
            SpeechEvent::Status { .. } | SpeechEvent::Speaker { .. } => {}
        }
    }

    /// The cloud stream is unusable (error, drop, or stalled when `reason` is None). With auto-failover the
    /// on-device text takes over (Electron's behaviour); without it the dictation ends now with `offline`.
    fn cloud_failed(&self, g: &mut Inner, reason: Option<String>) {
        let Some(a) = g.attempt.as_mut() else { return };
        if let Some(r) = &reason {
            a.arbiter.cloud_error(r);
        }
        let why = a.arbiter.fail_reason().unwrap_or("cloud failed").to_owned();
        tracing::warn!(engine = ?a.arbiter.cloud_engine().map(|e| e.as_str()), reason = %why, "cloud speech failed");
        a.session = None;
        if let Some(tx) = a.final_tx.take() {
            let _ = tx.send(Err(why));
            return;
        }
        // No fallback when the user turned failover off, or when there is no on-device stream to fall back to.
        if (!a.auto_failover || !a.local_wanted) && g.view.phase == TalkPhase::Listening {
            return self.go_offline(g);
        }
        self.refresh(g);
    }

    /// Auto-failover off and the cloud is gone: stop the mic and fail fast. Words the cloud heard go to the
    /// clipboard so nothing is lost.
    fn go_offline(&self, g: &mut Inner) {
        let heard = g
            .attempt
            .as_ref()
            .map(|a| a.arbiter.cloud_live())
            .unwrap_or_default();
        let msg = if heard.is_empty() {
            MSG_OFFLINE.to_owned()
        } else {
            self.deps().clipboard.write(&heard);
            format!("{MSG_OFFLINE}; what was heard is on the clipboard")
        };
        self.fail(g, TalkErrorKind::Offline, &msg, true);
    }

    fn spawn_stall_check(&self, id: String) {
        let me = Arc::downgrade(&self.0);
        let every = self.deps().timeouts.check.max(1);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(every)).await;
                let Some(sh) = me.upgrade() else { return };
                let svc = TalkService(sh);
                let now = svc.mono();
                let mut g = svc.lock();
                let Some(a) = g.attempt.as_mut().filter(|a| a.id == id) else {
                    return;
                };
                if a.arbiter.check(now) {
                    svc.cloud_failed(&mut g, None);
                }
            }
        });
    }

    // --------------------------------------------------------------------------------------------- lifecycle

    fn release_waiters(g: &mut Inner, id: &str) {
        for k in ["started", "stopped"] {
            if let Some(tx) = g.waiters.remove(&format!("{k}:{id}")) {
                let _ = tx.send(None);
            }
        }
    }

    fn refuse(&self, kind: TalkErrorKind, error: &str) -> TalkView {
        let mut g = self.lock();
        g.view = TalkView {
            phase: TalkPhase::Error,
            error: Some(error.into()),
            error_kind: Some(kind),
            ..g.view.idle_keeping()
        };
        self.emit_view(&g);
        g.view.clone()
    }

    fn fail(&self, g: &mut Inner, kind: TalkErrorKind, error: &str, cancel: bool) {
        if let Some(id) = g.view.id.clone() {
            if cancel {
                self.send(json!({"type": "dictation.cancel", "id": id}));
            }
            Self::release_waiters(g, &id);
        }
        g.attempt = None;
        g.view.phase = TalkPhase::Error;
        g.view.error = Some(error.to_owned());
        g.view.error_kind = Some(kind);
        g.view.level = 0.0;
        self.emit_view(g);
    }

    pub async fn start(&self) -> TalkView {
        if self.lock().view.active() {
            return self.state();
        }
        let cfg = self.cfg();
        let cloud = self.resolve_cloud(&cfg);
        let id = self.deps().ids.next_id();
        // The on-device session starts before the mic opens: `start()` is the engine's version handshake, so an
        // incompatible or missing module is a typed refusal with the mic never touched (warm-mic pre-roll covers
        // the spawn). With a cloud engine selected, the dictation goes ahead on the cloud alone.
        let local = match self.deps().local.start(Self::spec(&id)).await {
            Ok(sess) => Some(sess),
            Err(e) => {
                let (kind, msg) = start_error(&e);
                if cloud.is_none() {
                    return self.refuse(kind, &msg);
                }
                tracing::warn!(?kind, "on-device speech refused the session; cloud only");
                None
            }
        };
        let local_ok = local.is_some();
        let now = self.deps().clock.now_ms();
        let stall = if cfg.auto_failover {
            self.deps().timeouts.stall as f64
        } else {
            f64::INFINITY
        };
        let started_rx = {
            let mut g = self.lock();
            if g.view.active() {
                return g.view.clone();
            }
            g.dictation_helper = g.hellos.max(1);
            g.key_down = self.mono();
            g.first_partial = None;
            g.view = TalkView {
                phase: TalkPhase::Listening,
                id: Some(id.clone()),
                started_at: Some(now),
                engine: Some(
                    cloud
                        .as_ref()
                        .map(|(p, _)| p.engine())
                        .unwrap_or(Engine::OnDevice),
                ),
                ..TalkView::idle()
            };
            g.attempt = Some(Attempt {
                id: id.clone(),
                arbiter: Arbiter::new(stall),
                cloud,
                session: None,
                pending_audio: Vec::new(),
                cloud_val: TalkValidator::new(&[Channel::Mic]),
                local: None,
                local_val: TalkValidator::new(&[Channel::Mic]),
                local_done: None,
                local_wanted: local_ok,
                cloud_finals: Vec::new(),
                cloud_partial: String::new(),
                gen: 0,
                auto_failover: cfg.auto_failover,
                final_tx: None,
            });
            if let (Some(sess), Some(a)) = (local, g.attempt.as_mut()) {
                a.local = Some(drive(sess, Channel::Mic, self.on_event(&id, 0, true)));
            }
            Self::waiter(&mut g, format!("started:{id}"))
        };
        // Ask the helper first: the mic must open as early as possible; the socket connects in parallel.
        if !self.send(json!({"type": "dictation.start", "id": id})) {
            let mut g = self.lock();
            self.fail(
                &mut g,
                TalkErrorKind::HelperUnavailable,
                "the capture helper isn't running, so the mic can't open",
                false,
            );
            return g.view.clone();
        }
        {
            let mut g = self.lock();
            // The helper can answer at once (busy / mic denied): this attempt is already over.
            if g.view.id.as_deref() == Some(id.as_str()) && g.view.phase == TalkPhase::Listening {
                let dictionary = g.store.dictionary.clone();
                if let Some(a) = g.attempt.as_mut() {
                    self.connect_cloud(a, dictionary);
                }
                self.emit_view(&g);
                drop(g);
                self.spawn_stall_check(id.clone());
            }
        }
        let started = self
            .wait(format!("started:{id}"), started_rx, self.deps().timeouts.started)
            .await;
        let mut g = self.lock();
        if started.is_none()
            && g.view.id.as_deref() == Some(id.as_str())
            && g.view.phase == TalkPhase::Listening
        {
            self.fail(
                &mut g,
                TalkErrorKind::MicDidNotStart,
                "Microphone did not start",
                true,
            );
        }
        g.view.clone()
    }

    pub async fn stop(&self) -> TalkView {
        let (id, rx) = {
            let mut g = self.lock();
            let Some(id) = g.view.id.clone().filter(|_| g.view.phase == TalkPhase::Listening) else {
                return g.view.clone();
            };
            g.key_up = self.mono();
            g.view.phase = TalkPhase::Finalizing;
            g.view.level = 0.0;
            self.emit_view(&g);
            let rx = Self::waiter(&mut g, format!("stopped:{id}"));
            (id, rx)
        };
        self.send(json!({"type": "dictation.stop", "id": id}));
        let stopped = self
            .wait(format!("stopped:{id}"), rx, self.deps().timeouts.stopped)
            .await;
        let (text, dur) = {
            let g = self.lock();
            (
                stopped
                    .as_ref()
                    .map(|m| bound_final(&s(m, "text")))
                    .unwrap_or_default(),
                stopped
                    .as_ref()
                    .and_then(|m| m["durationMs"].as_f64())
                    .unwrap_or(g.key_up - g.key_down),
            )
        };
        let me = self.clone();
        tokio::spawn(async move { me.finish(id, text, dur).await });
        self.state()
    }

    pub fn cancel(&self) -> TalkView {
        let mut g = self.lock();
        let Some(id) = g.view.id.clone().filter(|_| g.view.active()) else {
            return g.view.clone();
        };
        self.send(json!({"type": "dictation.cancel", "id": id}));
        Self::release_waiters(&mut g, &id);
        g.attempt = None;
        g.view = g.view.idle_keeping();
        self.emit_view(&g);
        g.view.clone()
    }

    async fn ended_by_helper(
        &self,
        id: String,
        text: String,
        duration_ms: f64,
        kind: TalkErrorKind,
        msg: &str,
    ) {
        let dur = {
            let mut g = self.lock();
            if g.view.id.as_deref() != Some(id.as_str()) || g.view.phase != TalkPhase::Listening {
                return;
            }
            g.key_up = self.mono();
            g.view.phase = TalkPhase::Finalizing;
            g.view.level = 0.0;
            self.emit_view(&g);
            if duration_ms > 0.0 {
                duration_ms
            } else {
                g.key_up - g.key_down
            }
        };
        self.finish(id.clone(), text, dur).await;
        let mut g = self.lock();
        g.view.phase = TalkPhase::Error;
        g.view.error = Some(msg.to_owned());
        g.view.error_kind = Some(kind);
        g.view.level = 0.0;
        self.emit_view(&g);
    }

    /// Flush the cloud (if still in use), pick the text, then cleanup → insert → history.
    async fn finish(&self, id: String, local_text: String, duration_ms: f64) {
        let t = self.deps().timeouts;
        let rx = {
            let mut g = self.lock();
            let Some(a) = g.attempt.as_mut().filter(|a| a.id == id) else {
                return;
            };
            let local = a.local.as_ref().map(|l| {
                let (tx, rx) = oneshot::channel();
                a.local_done = Some(tx);
                l.finish();
                rx
            });
            let cloud = match (&a.session, a.arbiter.using_cloud()) {
                (Some(sess), true) => {
                    let (tx, rx) = oneshot::channel();
                    a.final_tx = Some(tx);
                    sess.finish();
                    Some(rx)
                }
                _ => None,
            };
            (local, cloud)
        };
        let (local_rx, rx) = rx;
        // The on-device engine's last finals (bounded by the same flush timeout as the cloud).
        let local_wait = async {
            if let Some(l) = local_rx {
                let _ = tokio::time::timeout(Duration::from_millis(t.final_ms), l).await;
            }
        };
        let cloud_wait = async {
            match rx {
                Some(rx) => Some(
                    match tokio::time::timeout(Duration::from_millis(t.final_ms), rx).await {
                        Ok(Ok(r)) => r,
                        Ok(Err(_)) => Err("cloud failed".into()),
                        Err(_) => Err("cloud final timed out".into()),
                    },
                ),
                None => None,
            }
        };
        let ((), cloud) = tokio::join!(local_wait, cloud_wait);
        let final_at = self.mono();
        let asr = {
            let mut g = self.lock();
            let Some(mut a) = g.attempt.take().filter(|a| a.id == id) else {
                return;
            };
            let cloud = if a.arbiter.using_cloud() {
                cloud.or(Some(Err("cloud failed".into())))
            } else {
                None
            };
            let had_cloud = a.arbiter.cloud_engine().is_some();
            let asr = a.arbiter.finish(&local_text, cloud);
            if had_cloud && !a.auto_failover && asr.engine == Engine::OnDevice {
                // Auto-failover off: never substitute the on-device text for the cloud the user picked.
                g.attempt = Some(a);
                self.go_offline(&mut g);
                return;
            }
            if let Some(r) = &asr.reason {
                tracing::info!(reason = %r, "used on-device text");
            }
            asr
        };
        if asr.text.trim().is_empty() {
            let mut g = self.lock();
            g.view = g.view.idle_keeping();
            self.emit_view(&g);
            return;
        }
        let cfg = self.cfg();
        let dictionary = {
            let mut g = self.lock();
            g.view.phase = TalkPhase::Cleaning;
            g.view.text = asr.text.clone();
            g.view.partial.clear();
            g.view.engine = Some(asr.engine);
            g.view.failover = Some(asr.failover);
            self.emit_view(&g);
            g.store.dictionary.clone()
        };
        let llm = if cfg.auto_cleanup && cfg.ai_cleanup && !cfg.local_only {
            self.deps().llm.llm().await
        } else {
            None
        };
        let clock = self.deps().clock.clone();
        let mono = move || clock.mono_ms();
        let cleaned = cleanup_dictation(
            &asr.text,
            &dictionary,
            cfg.auto_cleanup,
            llm.as_deref(),
            Duration::from_millis(cfg.cleanup_timeout_ms),
            &mono,
        )
        .await;
        if let Some(r) = &cleaned.fallback_reason {
            tracing::info!(reason = %r.chars().take(80).collect::<String>(), "talk cleanup fell back to rules");
        }
        let mut insert: Option<Value> = None;
        let mut error: Option<String> = None;
        if cfg.auto_insert {
            let rx = {
                let mut g = self.lock();
                g.view.phase = TalkPhase::Inserting;
                g.view.text = cleaned.text.clone();
                self.emit_view(&g);
                Self::waiter(&mut g, format!("insert:{id}"))
            };
            self.send(json!({"type": "insert.text", "id": id, "text": cleaned.text, "restoreClipboard": cfg.restore_clipboard}));
            insert = self.wait(format!("insert:{id}"), rx, t.insert).await;
            if insert.as_ref().map(|m| m["ok"] == true) != Some(true) {
                // Never lose the words: leave them on the clipboard to paste by hand.
                self.deps().clipboard.write(&cleaned.text);
                let why = insert
                    .as_ref()
                    .and_then(|m| m["error"].as_str())
                    .unwrap_or("insert timed out")
                    .to_owned();
                error = Some(format!("{why}; copied to clipboard"));
            }
        }
        let inserted_at = self.mono();
        let mut g = self.lock();
        let ok = insert.as_ref().map(|m| m["ok"] == true).unwrap_or(false);
        let method = insert
            .as_ref()
            .and_then(|m| serde_json::from_value::<InsertMethod>(m["method"].clone()).ok())
            .unwrap_or(InsertMethod::None);
        let entry = TalkEntry {
            id: id.clone(),
            at: g.view.started_at.unwrap_or_else(|| self.deps().clock.now_ms()),
            raw: asr.text.clone(),
            text: cleaned.text.clone(),
            cleanup: cleaned.method,
            engine: asr.engine,
            failover: asr.failover,
            insert: method,
            inserted: ok,
            app: insert.as_ref().and_then(|m| m["app"].as_str()).map(str::to_owned),
            duration_ms: duration_ms.max(0.0).round(),
            latency: Latency {
                first_partial_ms: g.first_partial.map(|f| (f - g.key_down).max(0.0).round()),
                final_ms: Some((final_at - g.key_up).max(0.0).round()),
                cleanup_ms: Some(cleaned.ms.max(0.0).round()),
                inserted_ms: ok.then(|| (inserted_at - g.key_up).max(0.0).round()),
            },
            error,
        };
        tracing::info!(
            engine = entry.engine.as_str(),
            first_partial_ms = ?entry.latency.first_partial_ms,
            final_ms = ?entry.latency.final_ms,
            inserted_ms = ?entry.latency.inserted_ms,
            "talk latency"
        );
        if cfg.save_history {
            g.store.history.insert(0, entry.clone());
            g.store.history.truncate(MAX_HISTORY);
            self.save(&g);
            self.deps().events.emit("talk:history-changed", json!({"id": id}));
        }
        g.view = TalkView {
            last: Some(entry),
            ..TalkView::idle()
        };
        self.emit_view(&g);
    }
}
