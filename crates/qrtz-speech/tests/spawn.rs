//! The on-device engine against the mock module (ADR 0046 §3, §6; review N55, N57-N59, N66, N76, N77): the real
//! spawner, confinement, protocol and validator, with the module installed from a signed bundle by the store.
#![cfg(target_os = "linux")]
mod support;
use qrtz_speech::confine::Confinement;
use qrtz_speech::download::{DownloadControl, Downloader};
use qrtz_speech::spawn::{ModuleHealth, ModuleSession, OnDeviceSpeech, SpawnOptions};
use qrtz_speech::update::{UpdateOutcome, Updater};
use qrtz_speech::{
    Channel, FailReason, SessionKind, SessionSpec, SpeakerMode, SpeechError, SpeechEvent, SpeechSession,
    SpeechStore,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::*;

struct Env {
    tmp: tempfile::TempDir,
    store: Arc<SpeechStore>,
    srv: Server,
    sk: ed25519_dalek::SigningKey,
    logs: Arc<Mutex<Vec<String>>>,
}

fn opts(tmp: &std::path::Path, logs: &Arc<Mutex<Vec<String>>>) -> SpawnOptions {
    let l = logs.clone();
    SpawnOptions {
        hello_timeout: Duration::from_secs(3),
        on_log: Some(Arc::new(move |m: &str| l.lock().unwrap().push(m.to_string()))),
        ..SpawnOptions::new(tmp.join("logs/speech.log"))
    }
}

async fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let (sk, pk) = key(1, "k1");
    let (_, rootpk) = key(9, "root");
    let store = Arc::new(SpeechStore::open(tmp.path().join("speech"), APP, pk, rootpk).unwrap());
    let srv = Server::start().await;
    // `now` is the wall clock here (the engine pins with it), so the list is issued now.
    let now = wall();
    let (d, s) = denylist_at(&sk, "k1", 1, now, now + 7 * DAY, &[]);
    srv.put("denylist.json", d);
    srv.put("denylist.json.sig", s);
    Env {
        tmp,
        store,
        srv,
        sk,
        logs: Default::default(),
    }
}

fn wall() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

impl Env {
    fn opts(&self) -> SpawnOptions {
        opts(self.tmp.path(), &self.logs)
    }
    /// Publish and install through the real updater, with the real golden-clip health check.
    async fn install(&self, version: &str, cfg: serde_json::Value) -> Result<UpdateOutcome, SpeechError> {
        let mut cfg = cfg;
        cfg["version"] = version.into();
        if cfg.get("say").is_none() {
            cfg["say"] = "the quick brown fox".into();
        }
        self.srv
            .publish(&mock_bundle(&self.sk, "k1", version, cfg, "The quick brown fox."));
        let health = ModuleHealth { opts: self.opts() };
        let now = || wall();
        Updater {
            store: &self.store,
            dl: &Downloader::new(&self.srv.base),
            health: &health,
            now_ms: &now,
        }
        .install_latest(false, &DownloadControl::default(), &|_| {})
        .await
    }
    fn engine(&self) -> OnDeviceSpeech {
        OnDeviceSpeech::new(self.store.clone(), self.opts())
    }
}

fn spec(id: &str, channels: &[Channel]) -> SessionSpec {
    SessionSpec {
        session_id: id.into(),
        kind: SessionKind::Recording,
        channels: channels.to_vec(),
        language_hint: None,
        speaker_mode: SpeakerMode::Fast,
    }
}

const SECOND: usize = 32_000;

async fn collect(s: &mut ModuleSession) -> Vec<SpeechEvent> {
    let mut out = vec![];
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = s.events().recv().await {
            let end = matches!(ev, SpeechEvent::Finished { .. } | SpeechEvent::Failed { .. });
            out.push(ev);
            if end {
                break;
            }
        }
    })
    .await;
    out
}

fn confined() -> bool {
    Confinement::probe().available()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_transcribes_both_channels_and_finishes() {
    let e = env().await;
    assert!(matches!(
        e.install("1.0.0", json!({})).await.unwrap(),
        UpdateOutcome::Installed { .. }
    ));
    let mut s = e
        .engine()
        .start_module(spec("s1", &[Channel::Mic, Channel::System]))
        .await
        .unwrap();
    assert_eq!(s.version(), "1.0.0");
    assert_eq!(s.confined, confined());
    s.push_audio(Channel::Mic, &vec![0u8; 2 * SECOND]).await.unwrap();
    s.push_audio(Channel::System, &vec![0u8; SECOND]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.finish().await.unwrap();
    let evs = collect(&mut s).await;
    let words: Vec<(Channel, String)> = evs
        .iter()
        .filter_map(|ev| match ev {
            SpeechEvent::Words {
                channel,
                is_final: true,
                ..
            } => Some((*channel, ev.text().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(words.iter().filter(|w| w.0 == Channel::Mic).count(), 2, "{evs:?}");
    assert_eq!(
        words.iter().filter(|w| w.0 == Channel::System).count(),
        1,
        "{evs:?}"
    );
    assert!(
        matches!(evs.last(), Some(SpeechEvent::Finished { duration_ms: 2000 })),
        "{evs:?}"
    );
    assert_eq!(e.store.state().failures, Vec::<u64>::new());
    // The module's stderr went to the 0600 log.
    let log = e.tmp.path().join("logs/speech.log");
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("qrtz-mock-speech 1.0.0"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n58_n59_n76_the_module_is_confined_and_holds_only_its_fds() {
    if !confined() {
        // N59: without Landlock + seccomp, release modules are refused.
        let e = env().await;
        let err = e.install("1.0.0", json!({"mode": "probe"})).await.unwrap_err();
        assert!(
            matches!(err, SpeechError::Health(ref m) if m.contains("confined")),
            "{err}"
        );
        return;
    }
    let e = env().await;
    e.install("1.0.0", json!({"mode": "probe"})).await.unwrap();
    e.logs.lock().unwrap().clear();
    let mut s = e
        .engine()
        .start_module(spec("s1", &[Channel::Mic]))
        .await
        .unwrap();
    s.finish().await.unwrap();
    let evs = collect(&mut s).await;
    assert!(
        matches!(evs.last(), Some(SpeechEvent::Finished { .. })),
        "exec was refused, the module lived: {evs:?}"
    );
    let probe = e
        .logs
        .lock()
        .unwrap()
        .iter()
        .find(|l| l.starts_with("probe "))
        .cloned()
        .unwrap();
    for blocked in [
        "inet=EPERM",
        "unix=EPERM",
        "passwd=EACCES",
        "home=EACCES",
        "tmp=EACCES",
        "kill1=EPERM",
        "killppid=EPERM",
        "ptrace=EPERM",
        "uring=EPERM",
        "vmread=EPERM",
        "prctl=EPERM",
        "exec=EACCES",
    ] {
        assert!(probe.contains(blocked), "{blocked} in {probe}");
    }
    // N58: exactly 0..7 (one model file) and nothing the host had open.
    assert!(probe.ends_with(" fds=0,1,2,3,4,5,6,7"), "{probe}");
    assert!(!std::path::Path::new("/tmp/qrtz-speech-probe").exists());
}

struct NoHealth;
#[async_trait::async_trait]
impl qrtz_speech::update::HealthCheck for NoHealth {
    async fn check(
        &self,
        _: &qrtz_speech::bundle::Manifest,
        _: qrtz_speech::files::ModuleFiles,
    ) -> Result<(), String> {
        Ok(())
    }
}

impl Env {
    /// Install a bundle without the clip check (for modules built to misbehave in a session).
    async fn install_unchecked(&self, version: &str, mut cfg: serde_json::Value) {
        cfg["version"] = version.into();
        self.srv.publish(&mock_bundle(&self.sk, "k1", version, cfg, "x"));
        let now = || wall();
        Updater {
            store: &self.store,
            dl: &Downloader::new(&self.srv.base),
            health: &NoHealth,
            now_ms: &now,
        }
        .install_latest(false, &DownloadControl::default(), &|_| {})
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_probes_do_see_an_unconfined_module() {
    // The regression test is only worth something if the probes detect access: unconfined, they must.
    let e = env().await;
    e.install_unchecked("1.0.0", json!({"mode": "probe", "probeUnsafe": false}))
        .await;
    let mut eng = e.engine();
    eng.opts.force_unconfined = true;
    let mut s = eng.start_module(spec("s1", &[Channel::Mic])).await.unwrap();
    assert!(!s.confined);
    s.finish().await.unwrap();
    collect(&mut s).await;
    let probe = e
        .logs
        .lock()
        .unwrap()
        .iter()
        .find(|l| l.starts_with("probe "))
        .cloned()
        .unwrap();
    assert!(
        probe.contains("passwd=ok") && probe.contains("inet=ok") && probe.contains("prctl=ok"),
        "{probe}"
    );
    // fd hygiene holds without the sandbox too (N58).
    assert!(probe.ends_with(" fds=0,1,2,3,4,5,6,7"), "{probe}");
    let _ = std::fs::remove_file("/tmp/qrtz-speech-probe");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n77_a_module_that_never_reads_never_blocks_the_writer() {
    let e = env().await;
    e.install_unchecked("1.0.0", json!({"mode": "never-reads"})).await;
    let mut s = e
        .engine()
        .start_module(spec("s1", &[Channel::Mic]))
        .await
        .unwrap();
    let t = Instant::now();
    for _ in 0..200 {
        s.push_audio(Channel::Mic, &vec![0u8; 3200]).await.unwrap(); // 100 ms frames, 20 s of audio
    }
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "writes never block: {:?}",
        t.elapsed()
    );
    assert!(
        s.dropped_ms() > 0,
        "the pipe filled and frames were dropped (counted)"
    );
    s.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_golden_clip_never_goes_live() {
    let e = env().await;
    e.install("1.0.0", json!({})).await.unwrap();
    let err = e
        .install("1.1.0", json!({"say": "something else entirely"}))
        .await
        .unwrap_err();
    assert!(matches!(err, SpeechError::Health(_)), "{err}");
    assert_eq!(e.store.state().current.as_deref(), Some("1.0.0"));
    let err = e.install("1.2.0", json!({"mode": "crash"})).await.unwrap_err();
    assert!(matches!(err, SpeechError::Health(_)), "{err}");
    assert_eq!(e.store.state().current.as_deref(), Some("1.0.0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn garbage_silence_and_a_lying_hello_fail() {
    let e = env().await;
    // Garbage after start: 3 violations fail the health-check session, so it never goes live.
    let err = e.install("1.0.0", json!({"mode": "garbage"})).await.unwrap_err();
    assert!(
        matches!(err, SpeechError::Health(ref m) if m.contains("ProtocolViolations")),
        "{err}"
    );
    // Silence: no hello in time.
    e.install_unchecked("1.0.0", json!({"mode": "silent"})).await;
    let mut eng = e.engine();
    eng.opts.hello_timeout = Duration::from_millis(500);
    let err = eng.start_module(spec("s1", &[Channel::Mic])).await.err().unwrap();
    assert!(
        matches!(err, SpeechError::Health(ref m) if m.contains("no hello")),
        "{err}"
    );
    // A module whose hello names another version than its signed manifest.
    let mut cfg = json!({"say": "the quick brown fox"});
    cfg["version"] = "9.9.9".into();
    e.srv
        .publish(&mock_bundle(&e.sk, "k1", "1.1.0", cfg, "the quick brown fox"));
    let now = || wall();
    let health = ModuleHealth { opts: e.opts() };
    let err = Updater {
        store: &e.store,
        dl: &Downloader::new(&e.srv.base),
        health: &health,
        now_ms: &now,
    }
    .install_latest(false, &DownloadControl::default(), &|_| {})
    .await
    .unwrap_err();
    assert!(
        matches!(err, SpeechError::Health(ref m) if m.contains("hello")),
        "{err}"
    );
    assert_eq!(e.store.state().current.as_deref(), Some("1.0.0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n66_a_session_on_a_version_revoked_mid_session_ends_at_its_next_safe_point() {
    let e = env().await;
    e.install("1.0.0", json!({})).await.unwrap();
    e.install("1.1.0", json!({})).await.unwrap();
    let mut s = e
        .engine()
        .start_module(spec("s1", &[Channel::Mic]))
        .await
        .unwrap();
    assert_eq!(s.version(), "1.1.0");
    let now = wall();
    let (d, sig) = denylist_at(&e.sk, "k1", 2, now, now + 7 * DAY, &["1.1.0"]);
    assert_eq!(
        e.store.apply_denylist(&d, &sig, now).unwrap(),
        None,
        "busy: no swap under the session"
    );
    s.push_audio(Channel::Mic, &vec![0u8; SECOND]).await.unwrap();
    let evs = collect(&mut s).await;
    assert!(
        evs.iter()
            .any(|ev| matches!(ev, SpeechEvent::Words { is_final: true, .. })),
        "the utterance in flight is kept: {evs:?}"
    );
    assert!(
        matches!(
            evs.last(),
            Some(SpeechEvent::Failed {
                reason: FailReason::Revoked
            })
        ),
        "{evs:?}"
    );
    drop(s);
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Idle: the next session runs on the clean N-1.
    let s2 = e
        .engine()
        .start_module(spec("s2", &[Channel::Mic]))
        .await
        .unwrap();
    assert_eq!(s2.version(), "1.0.0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_crashing_sessions_roll_back_to_n_minus_1() {
    let e = env().await;
    e.install("1.0.0", json!({})).await.unwrap();
    e.install_unchecked("1.1.0", json!({"mode": "crash"})).await;
    for i in 0..3 {
        // It dies right after hello: either `start` hits the closed pipe or the session fails as crashed.
        match e
            .engine()
            .start_module(spec(&format!("c{i}"), &[Channel::Mic]))
            .await
        {
            Ok(mut s) => {
                let evs = collect(&mut s).await;
                assert!(
                    matches!(
                        evs.last(),
                        Some(SpeechEvent::Failed {
                            reason: FailReason::Crashed
                        })
                    ),
                    "{evs:?}"
                );
            }
            Err(err) => assert!(matches!(err, SpeechError::Io(_)), "{err}"),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        e.store.state().current.as_deref(),
        Some("1.0.0"),
        "crash loop: back to the clean N-1"
    );
    assert!(e.store.is_quarantined("1.1.0"));
}
