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
    /// Recording part sessions: speech session id → that part's audio path (the module protocol caps session
    /// ids at 64 chars, so a path cannot be the id).
    recordings: Mutex<HashMap<String, String>>,
    pub timeout: Duration,
}

impl HelperAttach {
    pub fn new(helper: Option<Supervisor>) -> Arc<Self> {
        Arc::new(Self {
            helper,
            waiters: Mutex::default(),
            recordings: Mutex::default(),
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
    /// Bind a recording part session to its audio path before the session starts; unbound on attach.
    pub fn bind_recording(&self, session_id: &str, audio_path: &str) {
        self.recordings
            .lock()
            .unwrap()
            .insert(session_id.to_owned(), audio_path.to_owned());
    }
    /// The `speech.attach` line for a session: a dictation by id, a recording by its audio path (bound with
    /// `bind_recording`; an unbound recording session falls back to its id).
    /// N96: a recording session must have been bound to its file by the host (`bind_recording`); an unbound one
    /// fails closed (`None`) instead of naming a file from the session id: only the host names the file.
    pub fn attach_line(&self, spec: &SessionSpec, channels: &[Channel]) -> Option<Value> {
        let target = match spec.kind {
            SessionKind::Dictation => json!({"kind": "dictation", "id": spec.session_id}),
            _ => {
                let path = self.recordings.lock().unwrap().remove(&spec.session_id)?;
                json!({"kind": "recording", "audioPath": path})
            }
        };
        let chans: Vec<&str> = channels
            .iter()
            .map(|c| match c {
                Channel::Mic => "mic",
                Channel::System => "system",
            })
            .collect();
        Some(
            json!({"type": "speech.attach", "sessionId": spec.session_id, "target": target, "channels": chans}),
        )
    }
}

#[async_trait::async_trait]
impl AudioAttach for HelperAttach {
    async fn attach(&self, spec: &SessionSpec, fds: Vec<(Channel, OwnedFd)>) -> Result<(), SpeechError> {
        // N92: one attach per session id at a time. A second one would replace the first's waiter (its answer then
        // resolves the wrong attempt); refuse it instead, the first attempt keeps its waiter and its retry path.
        let (tx, rx) = oneshot::channel();
        {
            let mut w = self.waiters.lock().unwrap();
            if w.contains_key(&spec.session_id) {
                self.recordings.lock().unwrap().remove(&spec.session_id);
                return Err(SpeechError::Module(
                    "speech.attach already in flight for this session".into(),
                ));
            }
            w.insert(spec.session_id.clone(), tx);
        }
        let fail = |e: SpeechError| {
            self.waiters.lock().unwrap().remove(&spec.session_id);
            self.recordings.lock().unwrap().remove(&spec.session_id);
            Err(e)
        };
        let Some(h) = &self.helper else {
            return fail(SpeechError::Module("no capture helper".into()));
        };
        let channels: Vec<Channel> = fds.iter().map(|(c, _)| *c).collect();
        let Some(msg) = self.attach_line(spec, &channels) else {
            return fail(SpeechError::Module(
                "speech.attach: recording session not bound to a file".into(),
            ));
        };
        let line = match encode_host_message(&msg) {
            Ok(l) => l,
            Err(e) => return fail(SpeechError::Module(format!("speech.attach: {e}"))),
        };
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

// ------------------------------------------------------------------------------------------------ note routing

use qrtz_lifecycle::runtime::{HelperCommand, HelperPort, NoteRuntime};
use qrtz_speech::spawn::{OnDeviceSpeech, SpawnOptions};
use qrtz_speech::{
    choose_engine, App, CloudConsent, EngineChoice, FailReason, SpeakerMode, SpeechEngine, SpeechEvent,
    SpeechSession, SpeechStore,
};
use tokio::sync::mpsc;

/// The installed speech module (ADR 0046): its store, spawn options and update server. `None` in `AppBuild`
/// = this build has no speech module configured (the helper's own recogniser lines are used, transitional).
#[derive(Clone)]
pub struct SpeechSetup {
    pub store: Arc<SpeechStore>,
    pub opts: SpawnOptions,
    pub base_url: String,
}

impl SpeechSetup {
    /// The build's speech module source: `QRTZ_SPEECH_URL`, the built-in signing key `QRTZ_SPEECH_KEY_ID` /
    /// `QRTZ_SPEECH_KEY` (hex Ed25519) and the root key `QRTZ_SPEECH_ROOT_KEY_ID` / `QRTZ_SPEECH_ROOT_KEY`.
    /// The shell bakes these in at compile time (a dev or E2E build may override them at run time under the
    /// dev-knob rule, N87). `Ok(None)` = none set: no speech module in this build. Partly set or a bad key is an
    /// error (a misconfigured release must not silently ship without its module).
    pub fn from_vars(
        data_dir: &std::path::Path,
        app_version: &str,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, String> {
        const VARS: [&str; 5] = [
            "QRTZ_SPEECH_URL",
            "QRTZ_SPEECH_KEY_ID",
            "QRTZ_SPEECH_KEY",
            "QRTZ_SPEECH_ROOT_KEY_ID",
            "QRTZ_SPEECH_ROOT_KEY",
        ];
        let v: Vec<Option<String>> = VARS.iter().map(|k| get(k).filter(|s| !s.is_empty())).collect();
        if v.iter().all(Option::is_none) {
            return Ok(None);
        }
        let missing: Vec<&str> = VARS
            .iter()
            .zip(&v)
            .filter(|(_, x)| x.is_none())
            .map(|(k, _)| *k)
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "speech module partly configured; missing {}",
                missing.join(", ")
            ));
        }
        let v: Vec<String> = v.into_iter().map(Option::unwrap).collect();
        let builtin = qrtz_speech::bundle::PinnedKey::from_hex(&v[1], &v[2])
            .map_err(|e| format!("QRTZ_SPEECH_KEY: {e}"))?;
        let root = qrtz_speech::bundle::PinnedKey::from_hex(&v[3], &v[4])
            .map_err(|e| format!("QRTZ_SPEECH_ROOT_KEY: {e}"))?;
        let store = SpeechStore::open(data_dir.join("speech"), app_version, builtin, root)
            .map_err(|e| e.to_string())?;
        Ok(Some(Self {
            store: Arc::new(store),
            opts: SpawnOptions::new(data_dir.join("logs/speech.log")),
            base_url: v[0].clone(),
        }))
    }
}

enum Ctl {
    Finish,
    Cancel,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Why a session could not start, as the runtime's per-part failure (N42).
pub fn fail_reason(e: &SpeechError) -> FailReason {
    match e {
        SpeechError::NotInstalled => FailReason::NotInstalled,
        SpeechError::Denied(_) => FailReason::Revoked,
        // N96: an expired revocation list is "connect to update", like a stale one (not a revoked release).
        SpeechError::RevocationExpired | SpeechError::Stale { .. } => FailReason::Stale,
        _ => FailReason::StartFailed,
    }
}

/// Step 9 / N84 for note: one module session per recording part (session id = the part's audio path, both
/// channels), started and attached before the helper gets `recording.start` (so sample 0 is the part's start),
/// every event forwarded with `NoteRuntime::speech_event(Some(path), channels, ev)`, finished after the helper's
/// `recording.stopped` (all audio written, fds closed) and cancelled on `recording.aborted`. Note never uses a
/// cloud engine (the ElevenLabs talk opt-in never covers note, ADR 0046 §9 / N84): when on-device speech is not
/// available the part gets `Failed {NotInstalled | Revoked | Stale}` (speech-unavailable plus the expired-list
/// notice; the audio is still recorded, never silently).
pub struct NoteSpeech {
    pub engine: Arc<dyn SpeechEngine>,
    pub store: Arc<SpeechStore>,
    pub config: crate::SharedConfig,
    attach: Arc<HelperAttach>,
    next: std::sync::atomic::AtomicU64,
    runtime: std::sync::OnceLock<NoteRuntime>,
    sessions: Mutex<HashMap<String, mpsc::UnboundedSender<Ctl>>>,
}

pub const NOTE_CHANNELS: [Channel; 2] = [Channel::Mic, Channel::System];

impl NoteSpeech {
    pub fn new(setup: &SpeechSetup, attach: Arc<HelperAttach>, config: crate::SharedConfig) -> Arc<Self> {
        let engine = OnDeviceSpeech::new(setup.store.clone(), setup.opts.clone()).with_attach(attach.clone());
        Arc::new(Self {
            engine: Arc::new(engine),
            store: setup.store.clone(),
            config,
            attach,
            next: Default::default(),
            runtime: Default::default(),
            sessions: Mutex::default(),
        })
    }
    pub fn set_runtime(&self, rt: NoteRuntime) {
        let _ = self.runtime.set(rt);
    }
    pub fn choice(&self) -> EngineChoice {
        let cfg = self.config.get();
        let consent = CloudConsent {
            talk: false,
            note: false,
            local_only: cfg["privacy"]["localOnly"].as_bool().unwrap_or(false),
        };
        choose_engine(&self.store.availability(now_ms()), consent, App::Note)
    }
    pub fn active(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }
    fn event(&self, path: &str, ev: SpeechEvent) {
        if let Some(rt) = self.runtime.get() {
            rt.speech_event(Some(path), &NOTE_CHANNELS, ev);
        }
    }
    /// A part's module session: a short id (≤ 64 chars, the protocol cap) bound to the part's audio path for
    /// the helper attach.
    fn spec(&self, path: &str) -> SessionSpec {
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let session_id = format!("rec-{}-{n}", now_ms());
        self.attach.bind_recording(&session_id, path);
        SessionSpec {
            session_id,
            kind: SessionKind::Recording,
            channels: NOTE_CHANNELS.to_vec(),
            language_hint: None,
            speaker_mode: SpeakerMode::Accurate,
        }
    }

    /// Start the part's session; resolves when it is attached (or failed). Then the caller sends recording.start.
    pub async fn start_part(self: &Arc<Self>, path: &str) {
        match self.choice() {
            EngineChoice::OnDevice => match self.engine.start(self.spec(path)).await {
                Ok(sess) => self.pump(path.to_owned(), sess),
                Err(e) => {
                    tracing::warn!(err = %e, "note speech session did not start; this part has no live transcript");
                    self.event(
                        path,
                        SpeechEvent::Failed {
                            reason: fail_reason(&e),
                        },
                    );
                }
            },
            // Note has no cloud engine (consent.note is never set), so `Cloud` cannot come back; treat it alike.
            EngineChoice::Unavailable { reason, list_expired } => {
                let r = match reason.as_str() {
                    "not-installed" => FailReason::NotInstalled,
                    _ if list_expired || list_expired_flag(&reason) => FailReason::Stale,
                    _ => FailReason::Revoked,
                };
                self.event(path, SpeechEvent::Failed { reason: r });
            }
            EngineChoice::Cloud { fallback_reason } => {
                let r = if fallback_reason == "not-installed" {
                    FailReason::NotInstalled
                } else {
                    FailReason::Revoked
                };
                self.event(path, SpeechEvent::Failed { reason: r });
            }
        }
    }

    fn pump(self: &Arc<Self>, path: String, mut sess: Box<dyn SpeechSession>) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.sessions.lock().unwrap().insert(path.clone(), tx);
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                let step = tokio::select! {
                    ev = sess.events().recv() => Some(Err(ev)),
                    c = rx.recv() => c.map(Ok),
                };
                match step {
                    Some(Err(Some(ev))) => {
                        let end = matches!(ev, SpeechEvent::Finished { .. } | SpeechEvent::Failed { .. });
                        me.event(&path, ev);
                        if end {
                            break;
                        }
                    }
                    Some(Err(None)) => break,
                    Some(Ok(Ctl::Finish)) => {
                        if let Err(e) = sess.finish().await {
                            tracing::warn!(err = %e, "note speech finish failed");
                        }
                    }
                    Some(Ok(Ctl::Cancel)) => {
                        sess.cancel().await;
                        break;
                    }
                    None => {
                        sess.cancel().await;
                        break;
                    }
                }
            }
            me.sessions.lock().unwrap().remove(&path);
        });
    }

    /// Helper lines: a part's audio is complete once `recording.stopped` names it; `recording.aborted` tears the
    /// session down; a new helper process (`hello`) ends every session the old one fed.
    pub fn on_helper_line(&self, m: &Value) {
        let path = m["audioPath"].as_str().unwrap_or_default();
        let s = self.sessions.lock().unwrap();
        match m["type"].as_str() {
            Some("recording.stopped") => {
                if let Some(tx) = s.get(path) {
                    let _ = tx.send(Ctl::Finish);
                }
            }
            Some("recording.aborted") => {
                if let Some(tx) = s.get(path) {
                    let _ = tx.send(Ctl::Cancel);
                }
            }
            Some("hello") => {
                for tx in s.values() {
                    let _ = tx.send(Ctl::Finish);
                }
            }
            _ => {}
        }
    }
}

/// `stale` always means the revocation list expired (the "connect to update speech" notice).
fn list_expired_flag(reason: &str) -> bool {
    reason == "stale"
}

/// The runtime's `HelperPort` with note speech in front of `recording.start` (the module session is up and
/// attached before capture begins).
pub struct RoutedHelper {
    pub inner: Arc<dyn HelperPort>,
    pub speech: Arc<NoteSpeech>,
}

impl HelperPort for RoutedHelper {
    fn send(&self, cmd: &HelperCommand) -> bool {
        if let HelperCommand::RecordingStart { audio_path } = cmd {
            let (inner, speech, cmd, path) = (
                self.inner.clone(),
                self.speech.clone(),
                cmd.clone(),
                audio_path.clone(),
            );
            tokio::spawn(async move {
                speech.start_part(&path).await;
                if !inner.send(&cmd) {
                    tracing::warn!("helper went away before recording.start");
                    if let Some(tx) = speech.sessions.lock().unwrap().get(&path) {
                        let _ = tx.send(Ctl::Cancel);
                    }
                }
            });
            return true;
        }
        self.inner.send(cmd)
    }
    fn restart(&self) {
        self.inner.restart()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str) -> SessionSpec {
        SessionSpec {
            session_id: id.into(),
            kind: SessionKind::Dictation,
            channels: vec![Channel::Mic],
            language_hint: None,
            speaker_mode: SpeakerMode::Accurate,
        }
    }

    /// N92: a concurrent attach with the same session id is refused and leaves the first attempt's waiter alone.
    #[tokio::test]
    async fn a_second_attach_for_the_same_session_is_refused_and_the_first_still_resolves() {
        let a = HelperAttach::new(None);
        let (tx, rx) = oneshot::channel();
        a.waiters.lock().unwrap().insert("s1".into(), tx);
        let e = a.attach(&spec("s1"), vec![]).await.unwrap_err();
        assert!(e.to_string().contains("in flight"), "{e}");
        a.on_line(&json!({"type": "speech.attached", "sessionId": "s1"}));
        assert_eq!(rx.await.unwrap(), Ok(()));
        // No helper: refused, and nothing is left registered for the id.
        assert!(a.attach(&spec("s2"), vec![]).await.is_err());
        assert!(a.waiters.lock().unwrap().is_empty());
    }

    /// N96: a recording session the host never bound to a file is refused (no file named from the session id);
    /// a bound one names exactly the host's file, once.
    #[tokio::test]
    async fn an_unbound_recording_session_fails_closed() {
        let a = HelperAttach::new(None);
        let mut rec = spec("rec-1-1");
        rec.kind = SessionKind::Recording;
        assert_eq!(a.attach_line(&rec, &[Channel::Mic]), None);
        a.bind_recording("rec-1-1", "/data/recordings/x.wav");
        let l = a.attach_line(&rec, &[Channel::Mic, Channel::System]).unwrap();
        assert_eq!(
            l["target"],
            json!({"kind": "recording", "audioPath": "/data/recordings/x.wav"})
        );
        assert_eq!(
            a.attach_line(&rec, &[Channel::Mic]),
            None,
            "the binding is used once"
        );
        assert!(a.attach(&rec, vec![]).await.is_err());
        assert!(a.waiters.lock().unwrap().is_empty());
    }

    /// N96: an expired revocation list asks to connect (stale), it is not a revoked release.
    #[test]
    fn an_expired_revocation_list_reads_as_stale() {
        assert!(matches!(
            fail_reason(&SpeechError::RevocationExpired),
            FailReason::Stale
        ));
        assert!(matches!(
            fail_reason(&SpeechError::Denied("x".into())),
            FailReason::Revoked
        ));
    }
}
