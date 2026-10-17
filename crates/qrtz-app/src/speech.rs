//! The speech module's audio comes from the helper (ADR 0046 §1, N55): per session the engine hands the write
//! ends here; they go to the helper as one SCM_RIGHTS message on the speech socket followed by `speech.attach`,
//! and the session starts only once the helper answers `speech.attached` (a rejection or no answer in time
//! fails the start; the host has dropped its copies either way).
use qrtz_helper_bridge::{encode_host_message, Supervisor};
use qrtz_speech::spawn::AudioAttach;
use qrtz_speech::{Channel, SessionKind, SessionSpec, SpeechError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

type Waiter = oneshot::Sender<Result<(), String>>;

pub struct HelperAttach {
    helper: Option<Supervisor>,
    waiters: Mutex<HashMap<String, Waiter>>,
    pub timeout: Duration,
}

impl HelperAttach {
    pub fn new(helper: Option<Supervisor>) -> Arc<Self> {
        Arc::new(Self {
            helper,
            waiters: Mutex::default(),
            timeout: Duration::from_secs(5),
        })
    }
    /// `speech.attached` / `speech.attach.rejected` from the helper.
    pub fn on_line(&self, m: &Value) {
        let sid = m["sessionId"].as_str().unwrap_or_default();
        let Some(w) = self.waiters.lock().unwrap().remove(sid) else {
            return;
        };
        let _ = w.send(match m["type"].as_str() {
            Some("speech.attached") => Ok(()),
            _ => Err(m["reason"].as_str().unwrap_or("rejected").to_owned()),
        });
    }
    /// The `speech.attach` line for a session: a dictation by id, a recording by its audio path (the session id
    /// of a recording part session is that part's audio path).
    pub fn attach_line(spec: &SessionSpec, channels: &[Channel]) -> Value {
        let target = match spec.kind {
            SessionKind::Dictation => json!({"kind": "dictation", "id": spec.session_id}),
            _ => json!({"kind": "recording", "audioPath": spec.session_id}),
        };
        let chans: Vec<&str> = channels
            .iter()
            .map(|c| match c {
                Channel::Mic => "mic",
                Channel::System => "system",
            })
            .collect();
        json!({"type": "speech.attach", "sessionId": spec.session_id, "target": target, "channels": chans})
    }
}

#[async_trait::async_trait]
impl AudioAttach for HelperAttach {
    async fn attach(&self, spec: &SessionSpec, fds: Vec<(Channel, OwnedFd)>) -> Result<(), SpeechError> {
        let Some(h) = &self.helper else {
            return Err(SpeechError::Module("no capture helper".into()));
        };
        let channels: Vec<Channel> = fds.iter().map(|(c, _)| *c).collect();
        let line = encode_host_message(&Self::attach_line(spec, &channels))
            .map_err(|e| SpeechError::Module(format!("speech.attach: {e}")))?;
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(spec.session_id.clone(), tx);
        let fds: Vec<OwnedFd> = fds.into_iter().map(|(_, f)| f).collect();
        if let Err(e) = h.send_fds(line, spec.session_id.as_bytes().to_vec(), fds).await {
            self.waiters.lock().unwrap().remove(&spec.session_id);
            return Err(SpeechError::Module(format!("speech.attach: {e}")));
        }
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(reason))) => Err(SpeechError::Module(format!(
                "helper refused the audio fds: {reason}"
            ))),
            _ => {
                self.waiters.lock().unwrap().remove(&spec.session_id);
                Err(SpeechError::Module(
                    "helper did not acknowledge speech.attach".into(),
                ))
            }
        }
    }
}
