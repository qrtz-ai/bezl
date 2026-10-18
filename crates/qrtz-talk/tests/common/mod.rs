//! Test harness: a scripted helper (admission through the shared helper arbiter), a paused tokio clock, the
//! in-process mock speech module, recording sinks. Nothing here touches the network.
#![allow(dead_code)]
use base64::Engine as _;
use qrtz_lifecycle::{arbiter_step, ArbiterCmd, ArbiterEvent, ArbiterState, Configure, Decision};
use qrtz_model::OsPermission;
use qrtz_talk::mock::MockSpeech;
use qrtz_talk::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

/// Paused tokio time as both clocks; wall = a fixed epoch + elapsed (jumps can be injected).
pub struct TestClock {
    t0: tokio::time::Instant,
    pub wall_offset: Mutex<f64>,
}
impl TestClock {
    pub fn new() -> Self {
        Self {
            t0: tokio::time::Instant::now(),
            wall_offset: Mutex::new(1_760_000_000_000.0),
        }
    }
}
impl Clock for TestClock {
    fn now_ms(&self) -> f64 {
        *self.wall_offset.lock().unwrap() + self.mono_ms()
    }
    fn mono_ms(&self) -> f64 {
        self.t0.elapsed().as_millis() as f64
    }
}

#[derive(Default)]
pub struct SeqIds(AtomicU64);
impl IdSource for SeqIds {
    fn next_id(&self) -> String {
        format!("t{}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

#[derive(Default)]
pub struct Events(pub Mutex<Vec<(String, Value)>>);
impl TalkEvents for Events {
    fn emit(&self, e: &str, p: Value) {
        self.0.lock().unwrap().push((e.into(), p));
    }
}

#[derive(Default)]
pub struct Clip(pub Mutex<Vec<String>>);
impl Clipboard for Clip {
    fn write(&self, t: &str) {
        self.0.lock().unwrap().push(t.into());
    }
}

/// FsIo that can be told to fail writes like a full disk (ENOSPC), after leaving a partial tmp behind.
#[derive(Default)]
pub struct FaultyIo {
    pub disk_full: std::sync::atomic::AtomicBool,
    pub writes: AtomicU64,
}
impl StoreIo for FaultyIo {
    fn read(&self, p: &Path) -> std::io::Result<Vec<u8>> {
        FsIo.read(p)
    }
    fn write_private_atomic(&self, p: &Path, b: &[u8]) -> std::io::Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if self.disk_full.load(Ordering::SeqCst) {
            // A real ENOSPC: write half into the tmp through the same path, then fail and clean up like the real one.
            let tmp = qrtz_talk::store::tmp_path(p);
            let _ = std::fs::write(&tmp, &b[..b.len() / 2]);
            let _ = std::fs::remove_file(&tmp);
            return Err(std::io::Error::from_raw_os_error(28));
        }
        FsIo.write_private_atomic(p, b)
    }
    fn quarantine(&self, p: &Path, to: &Path) -> std::io::Result<()> {
        FsIo.quarantine(p, to)
    }
    fn remove_stale_tmp(&self, p: &Path) -> std::io::Result<()> {
        FsIo.remove_stale_tmp(p)
    }
}

#[derive(Debug, Clone, Default)]
pub struct HelperOpts {
    pub mic_denied: bool,
    /// N134: refuse every dictation.start with error{dictation.asr} (no recogniser could take it).
    pub no_recogniser: bool,
    pub insert_fails: bool,
    /// Audio chunk period (ms) while dictating.
    pub audio_every_ms: u64,
    /// Never answer dictation.start.
    pub mute: bool,
    /// A qrtz speech module as the on-device engine: (protocol, version). `None` = today's helper (its
    /// `dictation.asr` lines through `HelperAsrSpeech`).
    pub module: Option<(u32, String)>,
    /// Answer `insert.text` this much later (a slow paste into a busy app).
    pub insert_delay_ms: u64,
    /// Send `dictation.stopped` this much later (a slow mic teardown / final flush in the helper).
    pub stopped_delay_ms: u64,
    /// N124: the helper's final has this many words past its last partial (the release tail and the recogniser's
    /// last chunk, which only its end-of-stream flush emits).
    pub final_tail_words: usize,
    /// N166: `dictation.started`'s `preRollMs` (the head an armed or warm mic heard before the start, ADR 0104).
    pub pre_roll_ms: u64,
    /// ADR 0112: the `dictation.level` values sent with each audio block, in turn (empty: always 0.2).
    pub levels: Vec<f64>,
}

struct HState {
    arb: ArbiterState,
    active: Option<String>,
    /// Words the on-device recogniser has heard in the active dictation.
    heard: usize,
    running: bool,
    sent: Vec<Value>,
    seq: u64,
    opts: HelperOpts,
    /// The mock module (the on-device engine) when `opts.module` is set.
    speech: Option<MockSpeech>,
    /// Every `dictation.stopped` text the helper sent (or will send, when delayed).
    finals: Vec<String>,
}

pub fn heard_text(n: usize) -> String {
    let w: Vec<&str> = qrtz_talk::mock::MOCK_DICTATION.split(' ').collect();
    w[..n.min(w.len())].join(" ")
}

/// The helper over stdio, scripted: dictation/recording admission and the mic tap come from the shared arbiter
/// (`arbiter_step`, the same rules as the Swift helper), lines go back to the service in order through one pump.
pub struct MockHelper {
    st: Mutex<HState>,
    out: OnceLock<UnboundedSender<Value>>,
    svc: OnceLock<TalkService>,
}

impl MockHelper {
    pub fn new(opts: HelperOpts) -> Arc<Self> {
        let denied = if opts.mic_denied {
            vec![OsPermission::Microphone]
        } else {
            vec![]
        };
        Arc::new(Self {
            st: Mutex::new(HState {
                arb: ArbiterState::initial(denied),
                active: None,
                heard: 0,
                running: true,
                sent: vec![],
                seq: 0,
                speech: opts.module.as_ref().map(|(p, v)| MockSpeech::installed(*p, v)),
                finals: vec![],
                opts,
            }),
            out: OnceLock::new(),
            svc: OnceLock::new(),
        })
    }
    pub fn attach(self: &Arc<Self>, svc: TalkService) {
        let (tx, mut rx) = unbounded_channel::<Value>();
        let _ = self.out.set(tx);
        let _ = self.svc.set(svc.clone());
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                svc.helper_line(&m);
            }
        });
    }
    pub fn speech(&self) -> Option<MockSpeech> {
        self.st.lock().unwrap().speech.clone()
    }
    /// A new module version is installed (running sessions keep theirs).
    pub fn module_update(&self, version: &str) {
        if let Some(m) = self.speech() {
            m.install(Some((
                qrtz_talk::qrtz_speech::protocol::SPEECH_PROTOCOL_VERSION,
                version,
            )));
        }
    }
    /// The module process of the active dictation crashes (the helper stays up and keeps the mic).
    pub fn module_crash(&self) {
        if let (Some(m), Some(id)) = (self.speech(), self.active()) {
            m.crash(&id);
        }
    }
    /// A raw module line for the active dictation (malformed / hostile output tests).
    pub fn module_line(&self, m: Value) {
        let (sp, id, audio) = {
            let st = self.st.lock().unwrap();
            (
                st.speech.clone(),
                st.active.clone(),
                st.heard as u64 * st.opts.audio_every_ms.max(1),
            )
        };
        if let (Some(sp), Some(id)) = (sp, id) {
            sp.line(&id, &m.to_string(), audio);
        }
    }
    pub fn active(&self) -> Option<String> {
        self.st.lock().unwrap().active.clone()
    }
    /// The push key (`hotkey` down / up from the helper's tap).
    pub fn key(&self, action: &str) {
        self.deliver(json!({"type": "hotkey", "action": action}));
    }
    pub fn deliver(&self, m: Value) {
        if let Some(tx) = self.out.get() {
            let _ = tx.send(m);
        }
    }
    pub fn sent(&self) -> Vec<Value> {
        self.st.lock().unwrap().sent.clone()
    }
    /// The helper's real final texts, in order.
    pub fn finals(&self) -> Vec<String> {
        self.st.lock().unwrap().finals.clone()
    }
    pub fn sent_of(&self, ty: &str) -> Vec<Value> {
        self.sent().into_iter().filter(|m| m["type"] == ty).collect()
    }
    pub fn arbiter(&self) -> ArbiterState {
        self.st.lock().unwrap().arb.clone()
    }
    fn step(st: &mut HState, e: ArbiterEvent) -> qrtz_lifecycle::ArbiterResult {
        let r = arbiter_step(&st.arb, &e);
        st.arb = r.state.clone();
        r
    }
    /// A dictation the host never heard of (it outlived a host reload).
    pub fn preload_dictation(&self, id: &str) {
        let mut st = self.st.lock().unwrap();
        Self::step(
            &mut st,
            ArbiterEvent::Cmd {
                cmd: ArbiterCmd::DictationStart,
                audio_path: None,
            },
        );
        st.active = Some(id.into());
    }
    /// The note side starts a recording on the same helper (mux).
    pub fn recording_start(&self, path: &str) -> qrtz_lifecycle::ArbiterResult {
        let mut st = self.st.lock().unwrap();
        let r = Self::step(
            &mut st,
            ArbiterEvent::Cmd {
                cmd: ArbiterCmd::RecordingStart,
                audio_path: Some(path.into()),
            },
        );
        drop(st);
        // Recording lines reach talk too in this test (qrtz-app only routes talk types; talk must ignore the rest).
        self.deliver(json!({"type": "recording.started", "audioPath": path}));
        r
    }
    pub fn recording_stop(&self) -> qrtz_lifecycle::ArbiterResult {
        let mut st = self.st.lock().unwrap();
        Self::step(
            &mut st,
            ArbiterEvent::Cmd {
                cmd: ArbiterCmd::RecordingStop,
                audio_path: None,
            },
        )
    }
    /// TCC: microphone access withdrawn now.
    pub fn revoke_mic(&self) {
        let mut st = self.st.lock().unwrap();
        let r = Self::step(
            &mut st,
            ArbiterEvent::Revoked {
                revoked: OsPermission::Microphone,
            },
        );
        if r.effects.iter().any(|e| e == "dictation.stop:permission-revoked") {
            if let Some(id) = st.active.take() {
                let text = heard_text(st.heard);
                drop(st);
                self.deliver(
                    json!({"type": "dictation.stopped", "id": id, "text": text, "durationMs": 0,
                                    "reason": "permission-revoked"}),
                );
            }
        }
    }
    /// The helper process dies: no more lines, sends fail, the supervisor reports it down.
    pub fn die(&self) {
        let mut st = self.st.lock().unwrap();
        Self::step(&mut st, ArbiterEvent::HelperLost { helper_lost: true });
        st.active = None;
        st.running = false;
        drop(st);
        if let Some(s) = self.svc.get() {
            s.helper_status(false, "crashed");
        }
    }
    /// A new helper process says hello (the old one's dictation is gone).
    pub fn restart(&self) {
        let mut st = self.st.lock().unwrap();
        let denied = st.arb.denied.clone();
        st.arb = ArbiterState::initial(denied);
        st.active = None;
        st.running = true;
        drop(st);
        self.deliver(
            json!({"type": "hello", "protocol": 1, "helperVersion": "test", "os": "test", "features": []}),
        );
    }

    fn audio_loop(self: Arc<Self>, id: String) {
        tokio::spawn(async move {
            loop {
                let every = self.st.lock().unwrap().opts.audio_every_ms.max(1);
                tokio::time::sleep(Duration::from_millis(every)).await;
                let mut st = self.st.lock().unwrap();
                if st.active.as_deref() != Some(id.as_str()) || !st.running {
                    return;
                }
                st.seq += 1;
                st.heard += 1;
                let (seq, heard) = (st.seq, st.heard);
                let speech = st.speech.clone();
                let level = match st.opts.levels.len() {
                    0 => 0.2,
                    n => st.opts.levels[(heard - 1) % n],
                };
                drop(st);
                let pcm = base64::engine::general_purpose::STANDARD.encode(vec![0u8; (every as usize) * 32]);
                self.deliver(json!({"type": "dictation.level", "rms": level}));
                self.deliver(json!({"type": "dictation.audio", "id": id, "seq": seq, "pcm16": pcm}));
                // The on-device recogniser: a growing partial.
                match speech {
                    None => self.deliver(json!({"type": "dictation.asr", "id": id, "event": {"kind": "partial",
                                                "utteranceId": "u1", "startMs": 0, "text": heard_text(heard)}})),
                    Some(sp) => {
                        // The module reads the mic from its fd; its words come back as module lines.
                        let e = every as usize;
                        let words: Vec<Value> = heard_text(heard)
                            .split(' ')
                            .enumerate()
                            .map(|(i, w)| json!({"text": w, "startMs": i * e, "endMs": i * e + e - 1}))
                            .collect();
                        let line = json!({"type": "words", "utteranceId": "u1", "channel": "mic", "final": false,
                                          "language": "en", "words": words});
                        sp.line(&id, &line.to_string(), (heard * e) as u64);
                    }
                }
            }
        });
    }
}

pub struct SinkHandle(pub Arc<MockHelper>);
impl HelperSink for SinkHandle {
    fn send(&self, m: &Value) -> bool {
        let me = self.0.clone();
        let mut st = me.st.lock().unwrap();
        if !st.running {
            return false;
        }
        st.sent.push(m.clone());
        let id = m["id"].as_str().unwrap_or_default().to_owned();
        match m["type"].as_str().unwrap_or_default() {
            "dictation.configure" => {
                let warm = m["warmMic"].as_bool().unwrap_or(false);
                MockHelper::step(
                    &mut st,
                    ArbiterEvent::Configure {
                        configure: Configure { warm_mic: warm },
                    },
                );
            }
            "dictation.start" => {
                if st.opts.mute {
                    return true;
                }
                if st.opts.no_recogniser {
                    drop(st);
                    me.deliver(json!({"type": "error", "code": "dictation.asr", "message": "no speech recogniser is ready",
                                      "fatal": false, "id": id}));
                    return true;
                }
                let r = MockHelper::step(
                    &mut st,
                    ArbiterEvent::Cmd {
                        cmd: ArbiterCmd::DictationStart,
                        audio_path: None,
                    },
                );
                match r.decision {
                    Decision::Accept => {
                        st.active = Some(id.clone());
                        st.heard = 0;
                        let pre = st.opts.pre_roll_ms;
                        drop(st);
                        me.deliver(json!({"type": "dictation.started", "id": id, "preRollMs": pre}));
                        me.clone().audio_loop(id);
                    }
                    Decision::Ignore => {
                        let active = st.active.clone();
                        drop(st);
                        me.deliver(json!({"type": "error", "code": "dictation.busy", "message": "a dictation is already running",
                                          "fatal": false, "id": id, "activeId": active}));
                    }
                    Decision::Refuse => {
                        drop(st);
                        me.deliver(json!({"type": "error", "code": "dictation.mic", "message": "microphone access denied", "fatal": false}));
                    }
                }
            }
            ty @ ("dictation.stop" | "dictation.cancel") => {
                if st.active.as_deref() == Some(id.as_str()) {
                    let cmd = if ty == "dictation.stop" {
                        ArbiterCmd::DictationStop
                    } else {
                        ArbiterCmd::DictationCancel
                    };
                    MockHelper::step(
                        &mut st,
                        ArbiterEvent::Cmd {
                            cmd,
                            audio_path: None,
                        },
                    );
                    st.active = None;
                    if ty == "dictation.stop" {
                        let text = heard_text(st.heard + st.opts.final_tail_words);
                        st.finals.push(text.clone());
                        let delay = st.opts.stopped_delay_ms;
                        drop(st);
                        let line =
                            json!({"type": "dictation.stopped", "id": id, "text": text, "durationMs": 1000});
                        if delay == 0 {
                            me.deliver(line);
                        } else {
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(delay)).await;
                                me.deliver(line);
                            });
                        }
                    }
                }
            }
            "insert.text" => {
                let (fails, delay) = (st.opts.insert_fails, st.opts.insert_delay_ms);
                drop(st);
                let line = if fails {
                    json!({"type": "insert.result", "id": id, "method": "none", "ok": false, "error": "no focused field"})
                } else {
                    json!({"type": "insert.result", "id": id, "method": "ax-selected-text", "ok": true, "app": "Mock Editor"})
                };
                if delay == 0 {
                    me.deliver(line);
                } else {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        me.deliver(line);
                    });
                }
            }
            _ => {}
        }
        true
    }
}

pub struct Rig {
    pub svc: TalkService,
    pub helper: Arc<MockHelper>,
    pub events: Arc<Events>,
    pub clip: Arc<Clip>,
    pub io: Arc<FaultyIo>,
    pub dir: tempfile::TempDir,
    pub store: PathBuf,
    pub config: Arc<Mutex<Value>>,
    /// The mock module, when `HelperOpts.module` is set.
    pub speech: Option<MockSpeech>,
}

pub struct RigOpts {
    pub helper: HelperOpts,
    pub config: Value,
    pub seed_store: Option<Vec<u8>>,
    pub timeouts: Timeouts,
    /// The on-device engine's `start` (module spawn + handshake) takes this long.
    pub local_start_delay_ms: u64,
    /// N165: the on-device session's `finish` (module flush) takes this long.
    pub local_finish_delay_ms: u64,
}
impl Default for RigOpts {
    fn default() -> Self {
        Self {
            helper: HelperOpts {
                audio_every_ms: 50,
                ..Default::default()
            },
            config: json!({}),
            seed_store: None,
            timeouts: Timeouts {
                started: 500,
                stopped: 500,
                stopped_per_audio_s: 0,
                stopped_max: 500,
                helper_tail: None,
                insert: 300,
                final_ms: 300,
            },
            local_start_delay_ms: 0,
            local_finish_delay_ms: 0,
        }
    }
}

/// An on-device engine whose `start` takes a while (the real one spawns the module process and handshakes).
pub struct SlowStart {
    pub inner: Arc<dyn qrtz_talk::qrtz_speech::SpeechEngine>,
    pub delay_ms: u64,
}
#[async_trait::async_trait]
impl qrtz_talk::qrtz_speech::SpeechEngine for SlowStart {
    fn info(&self) -> qrtz_talk::qrtz_speech::EngineInfo {
        self.inner.info()
    }
    async fn start(
        &self,
        spec: qrtz_talk::qrtz_speech::SessionSpec,
    ) -> Result<Box<dyn qrtz_talk::qrtz_speech::SpeechSession>, qrtz_talk::qrtz_speech::SpeechError> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        self.inner.start(spec).await
    }
}

/// An on-device engine whose session `finish` takes a while (a real module's end-of-stream flush).
pub struct SlowFinish {
    pub inner: Arc<dyn qrtz_talk::qrtz_speech::SpeechEngine>,
    pub delay_ms: u64,
}
struct SlowFinishSession {
    inner: Box<dyn qrtz_talk::qrtz_speech::SpeechSession>,
    delay_ms: u64,
}
#[async_trait::async_trait]
impl qrtz_talk::qrtz_speech::SpeechSession for SlowFinishSession {
    fn version(&self) -> &str {
        self.inner.version()
    }
    fn events(&mut self) -> &mut tokio::sync::mpsc::Receiver<qrtz_talk::qrtz_speech::SpeechEvent> {
        self.inner.events()
    }
    async fn finish(&mut self) -> Result<(), qrtz_talk::qrtz_speech::SpeechError> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        self.inner.finish().await
    }
    async fn cancel(&mut self) {
        self.inner.cancel().await
    }
}
#[async_trait::async_trait]
impl qrtz_talk::qrtz_speech::SpeechEngine for SlowFinish {
    fn info(&self) -> qrtz_talk::qrtz_speech::EngineInfo {
        self.inner.info()
    }
    async fn start(
        &self,
        spec: qrtz_talk::qrtz_speech::SessionSpec,
    ) -> Result<Box<dyn qrtz_talk::qrtz_speech::SpeechSession>, qrtz_talk::qrtz_speech::SpeechError> {
        let inner = self.inner.start(spec).await?;
        Ok(Box::new(SlowFinishSession {
            inner,
            delay_ms: self.delay_ms,
        }))
    }
}

pub fn rig(o: RigOpts) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("talk.json");
    if let Some(b) = &o.seed_store {
        std::fs::write(&store, b).unwrap();
    }
    let helper = MockHelper::new(o.helper);
    let events = Arc::new(Events::default());
    let clip = Arc::new(Clip::default());
    let io = Arc::new(FaultyIo::default());
    let config = Arc::new(Mutex::new(o.config));
    let cfg = config.clone();
    let speech = helper.speech();
    let helper_asr = Arc::new(HelperAsrSpeech::new());
    let local: Arc<dyn qrtz_talk::qrtz_speech::SpeechEngine> = match &speech {
        Some(m) => Arc::new(m.clone()),
        None => helper_asr.clone(),
    };
    let local: Arc<dyn qrtz_talk::qrtz_speech::SpeechEngine> = if o.local_start_delay_ms > 0 {
        Arc::new(SlowStart {
            inner: local,
            delay_ms: o.local_start_delay_ms,
        })
    } else {
        local
    };
    let local: Arc<dyn qrtz_talk::qrtz_speech::SpeechEngine> = if o.local_finish_delay_ms > 0 {
        Arc::new(SlowFinish {
            inner: local,
            delay_ms: o.local_finish_delay_ms,
        })
    } else {
        local
    };
    let svc = TalkService::new(TalkDeps {
        helper: Arc::new(SinkHandle(helper.clone())),
        events: events.clone(),
        clipboard: clip.clone(),
        clock: Arc::new(TestClock::new()),
        ids: Arc::new(SeqIds::default()),
        local,
        helper_asr,
        config: Arc::new(move || cfg.lock().unwrap().clone()),
        store_path: store.clone(),
        store_io: io.clone(),
        timeouts: o.timeouts,
    });
    helper.attach(svc.clone());
    Rig {
        svc,
        helper,
        events,
        clip,
        io,
        dir,
        store,
        config,
        speech,
    }
}

pub async fn until(f: impl Fn() -> bool, ms: u64) {
    let t0 = tokio::time::Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_millis(ms), "timed out waiting");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

pub async fn sleep(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await
}

impl Rig {
    pub fn view(&self) -> TalkView {
        self.svc.state()
    }
    pub fn phases(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for (e, p) in self.events.0.lock().unwrap().iter() {
            if e == "talk:state" {
                let ph = p["phase"].as_str().unwrap_or_default().to_owned();
                if out.last() != Some(&ph) {
                    out.push(ph);
                }
            }
        }
        out
    }
    /// Every emitted event and helper line as text (to prove a key never appears anywhere).
    pub fn everything_said(&self) -> String {
        format!(
            "{:?}{:?}{:?}",
            self.events.0.lock().unwrap(),
            self.helper.sent(),
            self.view()
        )
    }
    /// Dictate: start, wait for some text, stop, wait until the dictation is over.
    pub fn set_config(&self, v: Value) {
        *self.config.lock().unwrap() = v;
    }
    pub fn last_entry(&self) -> Option<TalkEntry> {
        self.svc.history().into_iter().next()
    }
    pub async fn dictate(&self, speak_ms: u64) -> TalkView {
        let me = self.svc.clone();
        tokio::spawn(async move { me.start().await });
        until(|| self.view().phase == TalkPhase::Listening, 1000).await;
        sleep(speak_ms).await;
        if self.view().phase == TalkPhase::Listening {
            self.svc.stop().await;
        }
        until(|| !self.view().active(), 10_000).await;
        self.view()
    }
}
