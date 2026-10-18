//! In-process mocks for tests and dev/e2e builds (never the network): [`MockSpeech`], a scripted qrtz speech
//! module behind the shared `SpeechEngine`.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// What the mock hears (packages/core/src/helper/mock.ts `MOCK_DICTATION`).
pub const MOCK_DICTATION: &str = "um so I I think we should ship cue arts talk on friday";

// ------------------------------------------------------------------------------------------------ speech module

/// A scripted qrtz speech module behind the shared `SpeechEngine`, doing what the host's on-device engine does with
/// a real module: the handshake at `start` (protocol, version), one session per dictation pinned to the version it
/// started on, every raw module line through `qrtz_speech_types::validate::OutputValidator` (violations drop the line, the
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
    tx: tokio::sync::mpsc::Sender<qrtz_speech_types::SpeechEvent>,
    validator: qrtz_speech_types::validate::OutputValidator,
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
        use qrtz_speech_types::validate::Verdict;
        let mut st = self.st.lock().unwrap();
        let Some(s) = st.sessions.get_mut(id) else { return };
        match s.validator.check(line, audio_ms, audio_ms) {
            Verdict::Event(e) => {
                let safe_point = matches!(e, qrtz_speech_types::SpeechEvent::Words { is_final: true, .. });
                let _ = s.tx.try_send(e);
                if safe_point && s.revoked {
                    let s = st.sessions.remove(id).unwrap();
                    let _ = s.tx.try_send(qrtz_speech_types::SpeechEvent::Failed {
                        reason: qrtz_speech_types::FailReason::Revoked,
                    });
                }
            }
            Verdict::Fail(why) => {
                tracing::warn!(%why, "mock speech module: protocol violations");
                let s = st.sessions.remove(id).unwrap();
                let _ = s.tx.try_send(qrtz_speech_types::SpeechEvent::Failed {
                    reason: qrtz_speech_types::FailReason::ProtocolViolations,
                });
            }
            Verdict::Ignore | Verdict::Dropped(_) => {}
        }
    }
    /// The module process dies.
    pub fn crash(&self, id: &str) {
        if let Some(s) = self.st.lock().unwrap().sessions.remove(id) {
            let _ = s.tx.try_send(qrtz_speech_types::SpeechEvent::Failed {
                reason: qrtz_speech_types::FailReason::Crashed,
            });
        }
    }
}

struct MockSpeechHandle {
    id: String,
    version: String,
    st: Arc<Mutex<SpeechState>>,
    rx: tokio::sync::mpsc::Receiver<qrtz_speech_types::SpeechEvent>,
}

#[async_trait::async_trait]
impl qrtz_speech_types::SpeechSession for MockSpeechHandle {
    fn version(&self) -> &str {
        &self.version
    }
    fn events(&mut self) -> &mut tokio::sync::mpsc::Receiver<qrtz_speech_types::SpeechEvent> {
        &mut self.rx
    }
    async fn finish(&mut self) -> Result<(), qrtz_speech_types::SpeechError> {
        if let Some(s) = self.st.lock().unwrap().sessions.remove(&self.id) {
            let _ =
                s.tx.try_send(qrtz_speech_types::SpeechEvent::Finished { duration_ms: 0 });
        }
        Ok(())
    }
    async fn cancel(&mut self) {
        self.st.lock().unwrap().sessions.remove(&self.id);
    }
}

#[async_trait::async_trait]
impl qrtz_speech_types::SpeechEngine for MockSpeech {
    fn info(&self) -> qrtz_speech_types::EngineInfo {
        qrtz_speech_types::EngineInfo {
            version: self.current().unwrap_or_default(),
            kind: qrtz_speech_types::EngineKind::OnDevice,
        }
    }
    async fn start(
        &self,
        spec: qrtz_speech_types::SessionSpec,
    ) -> Result<Box<dyn qrtz_speech_types::SpeechSession>, qrtz_speech_types::SpeechError> {
        let mut st = self.st.lock().unwrap();
        let Some((p, v)) = st.installed.clone() else {
            return Err(qrtz_speech_types::SpeechError::NotInstalled);
        };
        if st.denied.contains(&v) {
            return Err(qrtz_speech_types::SpeechError::Denied(v));
        }
        if p != qrtz_speech_types::protocol::SPEECH_PROTOCOL_VERSION {
            return Err(qrtz_speech_types::SpeechError::Incompatible(format!(
                "protocol {p}"
            )));
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
