//! In-process mock speech servers for tests and dev/e2e builds (never the network). [`MockCloud`] is a
//! [`WsConnector`] that answers each request in the ElevenLabs protocol, with a scriptable failure mode per
//! engine and an
//! "offline" switch that refuses new connections and drops live ones.
use crate::contract::Engine;
use crate::ws::{WsConn, WsConnector, WsIn, WsOut, WsRequest};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

/// What the mock hears (packages/core/src/helper/mock.ts `MOCK_DICTATION`).
pub const MOCK_DICTATION: &str = "um so I I think we should ship cue arts talk on friday";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockMode {
    /// Partials grow by a word per audio frame; the flush returns the whole sentence.
    Echo,
    /// The connection fails (DNS / refused): error then close 1006.
    Refuse,
    /// Opens and never answers.
    Hang,
    /// Echo for this many audio frames, then the socket drops (1006).
    DropAfter(usize),
    /// Echo, but the flush is never answered.
    NoFinal,
    /// Echo, then a provider error frame after this many audio frames.
    ErrorAfter(usize),
}

#[derive(Debug, Clone)]
pub struct MockRequest {
    pub engine: Engine,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub protocols: Vec<String>,
}

#[derive(Default)]
struct State {
    modes: HashMap<Engine, MockMode>,
    requests: Vec<MockRequest>,
    audio_frames: HashMap<usize, usize>,
    live: Vec<UnboundedSender<WsIn>>,
}

#[derive(Clone, Default)]
pub struct MockCloud {
    st: Arc<Mutex<State>>,
    offline: Arc<AtomicBool>,
}

/// ElevenLabs is the only cloud engine, so every request is one.
pub fn engine_of(_url: &str) -> Engine {
    Engine::Elevenlabs
}

impl MockCloud {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(self, e: Engine, m: MockMode) -> Self {
        self.set(e, m);
        self
    }
    pub fn set(&self, e: Engine, m: MockMode) {
        self.st.lock().unwrap().modes.insert(e, m);
    }
    /// Offline: every new connection is refused and every live one drops now.
    pub fn set_offline(&self, off: bool) {
        self.offline.store(off, Ordering::SeqCst);
        if off {
            for tx in self.st.lock().unwrap().live.drain(..) {
                let _ = tx.send(WsIn::Error("network is unreachable".into()));
                let _ = tx.send(WsIn::Closed {
                    code: Some(1006),
                    reason: "network is unreachable".into(),
                });
            }
        }
    }
    pub fn requests(&self) -> Vec<MockRequest> {
        self.st.lock().unwrap().requests.clone()
    }
    pub fn engines(&self) -> Vec<Engine> {
        self.requests().iter().map(|r| r.engine).collect()
    }
    /// Audio frames the n-th connection received.
    pub fn audio_frames(&self, n: usize) -> usize {
        self.st.lock().unwrap().audio_frames.get(&n).copied().unwrap_or(0)
    }
}

fn reply(text: &str, fin: bool) -> Vec<String> {
    let kind = if fin {
        "committed_transcript"
    } else {
        "partial_transcript"
    };
    vec![json!({"message_type": kind, "text": text}).to_string()]
}

fn is_audio(f: &WsOut) -> bool {
    match f {
        WsOut::Text(t) => serde_json::from_str::<Value>(t).is_ok_and(|v| v["commit"] == false),
        WsOut::Binary(_) => true,
        _ => false,
    }
}

fn is_flush(f: &WsOut) -> bool {
    let WsOut::Text(t) = f else { return false };
    serde_json::from_str::<Value>(t).is_ok_and(|v| v["commit"] == true)
}

impl WsConnector for MockCloud {
    fn connect(&self, req: WsRequest) -> WsConn {
        let engine = engine_of(&req.url);
        let (out_tx, mut out_rx) = unbounded_channel::<WsOut>();
        let (in_tx, in_rx) = unbounded_channel::<WsIn>();
        let (n, mode) = {
            let mut st = self.st.lock().unwrap();
            st.requests.push(MockRequest {
                engine,
                url: req.url.clone(),
                headers: req.headers.clone(),
                protocols: req.protocols.clone(),
            });
            st.live.push(in_tx.clone());
            (
                st.requests.len() - 1,
                st.modes.get(&engine).copied().unwrap_or(MockMode::Echo),
            )
        };
        let offline = self.offline.load(Ordering::SeqCst);
        let st = self.st.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if offline || mode == MockMode::Refuse {
                let _ = in_tx.send(WsIn::Error("connect failed: connection refused".into()));
                let _ = in_tx.send(WsIn::Closed {
                    code: Some(1006),
                    reason: "connection refused".into(),
                });
                return;
            }
            let _ = in_tx.send(WsIn::Open);
            let words: Vec<&str> = MOCK_DICTATION.split(' ').collect();
            let mut frames = 0usize;
            while let Some(f) = out_rx.recv().await {
                if f == WsOut::Close {
                    break;
                }
                if mode == MockMode::Hang {
                    continue;
                }
                if is_audio(&f) {
                    frames += 1;
                    st.lock().unwrap().audio_frames.insert(n, frames);
                    if let MockMode::DropAfter(k) = mode {
                        if frames > k {
                            let _ = in_tx.send(WsIn::Closed {
                                code: Some(1006),
                                reason: "dropped".into(),
                            });
                            return;
                        }
                    }
                    if let MockMode::ErrorAfter(k) = mode {
                        if frames > k {
                            let err = json!({"message_type": "error", "error": "quota exceeded"});
                            let _ = in_tx.send(WsIn::Text(err.to_string()));
                            continue;
                        }
                    }
                    let t = words[..frames.min(words.len())].join(" ");
                    for m in reply(&t, false) {
                        let _ = in_tx.send(WsIn::Text(m));
                    }
                } else if is_flush(&f) && mode != MockMode::NoFinal {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    for m in reply(MOCK_DICTATION, true) {
                        let _ = in_tx.send(WsIn::Text(m));
                    }
                }
            }
            let _ = in_tx.send(WsIn::Closed {
                code: Some(1000),
                reason: String::new(),
            });
        });
        WsConn {
            tx: out_tx,
            rx: in_rx,
        }
    }
}

// ------------------------------------------------------------------------------------------------ speech module

/// A scripted qrtz speech module behind the shared `SpeechEngine`, doing what the host's on-device engine does with
/// a real module: the handshake at `start` (protocol, version), one session per dictation pinned to the version it
/// started on, every raw module line through `qrtz_speech::validate::OutputValidator` (violations drop the line, the
/// third fails the session), a crash fails the session.
#[derive(Clone, Default)]
pub struct MockSpeech {
    st: Arc<Mutex<SpeechState>>,
}

#[derive(Default)]
struct SpeechState {
    /// (protocol, version) installed; `None` = not downloaded.
    installed: Option<(u32, String)>,
    sessions: HashMap<String, MockSpeechSession>,
    /// The last good version before the current one (a rollback target).
    previous: Option<(u32, String)>,
    /// Versions an applied revocation list denies.
    denied: Vec<String>,
    /// (session id, pinned version) in start order.
    started: Vec<(String, String)>,
}

struct MockSpeechSession {
    version: String,
    tx: tokio::sync::mpsc::Sender<qrtz_speech::SpeechEvent>,
    validator: qrtz_speech::validate::OutputValidator,
    /// The pinned version was revoked: the session ends at its next safe point (after the utterance in flight).
    revoked: bool,
}

impl MockSpeech {
    pub fn installed(protocol: u32, version: &str) -> Self {
        let m = Self::default();
        m.install(Some((protocol, version)));
        m
    }
    /// Install / update / remove the module. Running sessions keep their version (no swap mid-session).
    pub fn install(&self, v: Option<(u32, &str)>) {
        let mut st = self.st.lock().unwrap();
        let old = st.installed.take();
        st.previous = old.or(st.previous.take());
        st.installed = v.map(|(p, s)| (p, s.to_owned()));
    }
    /// A revocation list denies the current version, like `SpeechStore`: with a fresh list the store rolls back
    /// to the previous good version; with an expired list there is no rollback (fail closed, ADR 0046 §4b), so
    /// new sessions are refused with `Denied`. Running sessions on it end at their next safe point.
    pub fn revoke_current(&self, list_expired: bool) {
        let mut st = self.st.lock().unwrap();
        let Some((_, v)) = st.installed.clone() else {
            return;
        };
        st.denied.push(v.clone());
        for s in st.sessions.values_mut().filter(|s| s.version == v) {
            s.revoked = true;
        }
        if !list_expired {
            if let Some(prev) = st.previous.take().filter(|(_, pv)| !st.denied.contains(pv)) {
                st.installed = Some(prev);
            }
        }
    }
    pub fn current(&self) -> Option<String> {
        self.st.lock().unwrap().installed.clone().map(|(_, v)| v)
    }
    pub fn started(&self) -> Vec<(String, String)> {
        self.st.lock().unwrap().started.clone()
    }
    pub fn live(&self, id: &str) -> bool {
        self.st.lock().unwrap().sessions.contains_key(id)
    }
    /// The pinned version of a running session.
    pub fn version_of(&self, id: &str) -> Option<String> {
        self.st
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .map(|s| s.version.clone())
    }
    /// One raw module → host line for session `id` (`audio_ms` = audio written so far).
    pub fn line(&self, id: &str, line: &str, audio_ms: u64) {
        use qrtz_speech::validate::Verdict;
        let mut st = self.st.lock().unwrap();
        let Some(s) = st.sessions.get_mut(id) else { return };
        match s.validator.check(line, audio_ms, audio_ms) {
            Verdict::Event(e) => {
                let safe_point = matches!(e, qrtz_speech::SpeechEvent::Words { is_final: true, .. });
                let _ = s.tx.try_send(e);
                if safe_point && s.revoked {
                    let s = st.sessions.remove(id).unwrap();
                    let _ = s.tx.try_send(qrtz_speech::SpeechEvent::Failed {
                        reason: qrtz_speech::FailReason::Revoked,
                    });
                }
            }
            Verdict::Fail(why) => {
                tracing::warn!(%why, "mock speech module: protocol violations");
                let s = st.sessions.remove(id).unwrap();
                let _ = s.tx.try_send(qrtz_speech::SpeechEvent::Failed {
                    reason: qrtz_speech::FailReason::ProtocolViolations,
                });
            }
            Verdict::Ignore | Verdict::Dropped(_) => {}
        }
    }
    /// The module process dies.
    pub fn crash(&self, id: &str) {
        if let Some(s) = self.st.lock().unwrap().sessions.remove(id) {
            let _ = s.tx.try_send(qrtz_speech::SpeechEvent::Failed {
                reason: qrtz_speech::FailReason::Crashed,
            });
        }
    }
}

struct MockSpeechHandle {
    id: String,
    version: String,
    st: Arc<Mutex<SpeechState>>,
    rx: tokio::sync::mpsc::Receiver<qrtz_speech::SpeechEvent>,
}

#[async_trait::async_trait]
impl qrtz_speech::SpeechSession for MockSpeechHandle {
    fn version(&self) -> &str {
        &self.version
    }
    fn events(&mut self) -> &mut tokio::sync::mpsc::Receiver<qrtz_speech::SpeechEvent> {
        &mut self.rx
    }
    async fn finish(&mut self) -> Result<(), qrtz_speech::SpeechError> {
        if let Some(s) = self.st.lock().unwrap().sessions.remove(&self.id) {
            let _ =
                s.tx.try_send(qrtz_speech::SpeechEvent::Finished { duration_ms: 0 });
        }
        Ok(())
    }
    async fn cancel(&mut self) {
        self.st.lock().unwrap().sessions.remove(&self.id);
    }
}

#[async_trait::async_trait]
impl qrtz_speech::SpeechEngine for MockSpeech {
    fn info(&self) -> qrtz_speech::EngineInfo {
        qrtz_speech::EngineInfo {
            version: self.current().unwrap_or_default(),
            kind: qrtz_speech::EngineKind::OnDevice,
        }
    }
    async fn start(
        &self,
        spec: qrtz_speech::SessionSpec,
    ) -> Result<Box<dyn qrtz_speech::SpeechSession>, qrtz_speech::SpeechError> {
        let mut st = self.st.lock().unwrap();
        let Some((p, v)) = st.installed.clone() else {
            return Err(qrtz_speech::SpeechError::NotInstalled);
        };
        if st.denied.contains(&v) {
            return Err(qrtz_speech::SpeechError::Denied(v));
        }
        if p != qrtz_speech::protocol::SPEECH_PROTOCOL_VERSION {
            return Err(qrtz_speech::SpeechError::Incompatible(format!("protocol {p}")));
        }
        let (tx, rx) = tokio::sync::mpsc::channel(512);
        st.sessions.insert(
            spec.session_id.clone(),
            MockSpeechSession {
                version: v.clone(),
                tx,
                validator: Default::default(),
                revoked: false,
            },
        );
        st.started.push((spec.session_id.clone(), v.clone()));
        Ok(Box::new(MockSpeechHandle {
            id: spec.session_id,
            version: v,
            st: self.st.clone(),
            rx,
        }))
    }
}
