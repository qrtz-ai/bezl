//! Speech module fixtures for qrtz-app E2E tests: the real `qrtz-mock-speech` module in a signed bundle, served
//! by the qrtz-speech test kit's bundle server and installed through the real updater + health check.
#![allow(dead_code)]
use qrtz_speech::download::{DownloadControl, Downloader};
use qrtz_speech::spawn::{ModuleHealth, SpawnOptions};
use qrtz_speech::testkit::*;
use qrtz_speech::update::{UpdateOutcome, Updater};
use qrtz_speech::SpeechStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub fn wall() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub fn bin(name: &str, package: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let p = exe.parent().unwrap().parent().unwrap().join(name);
    let st = std::process::Command::new(env!("CARGO"))
        .args(["build", "-q", "-p", package, "--bin", name])
        .status()
        .unwrap();
    assert!(st.success());
    p
}

pub struct SpeechEnv {
    pub store: Arc<SpeechStore>,
    pub srv: Server,
    pub sk: ed25519_dalek::SigningKey,
    pub opts: SpawnOptions,
}

pub async fn speech_env(dir: &Path) -> SpeechEnv {
    let (sk, pk) = key(1, "k1");
    let (_, rootpk) = key(9, "root");
    let store = Arc::new(SpeechStore::open(dir.join("speech"), APP, pk, rootpk).unwrap());
    let srv = Server::start().await;
    let now = wall();
    let (d, s) = denylist_at(&sk, "k1", 1, now, now + 7 * DAY, &[]);
    srv.put("denylist.json", d);
    srv.put("denylist.json.sig", s);
    let opts = SpawnOptions {
        hello_timeout: Duration::from_secs(5),
        ..SpawnOptions::new(dir.join("logs/speech.log"))
    };
    SpeechEnv { store, srv, sk, opts }
}

impl SpeechEnv {
    pub fn publish(&self, version: &str, say: &str) {
        let entry = std::fs::read(bin("qrtz-mock-speech", "qrtz-speech")).unwrap();
        let cfg = serde_json::json!({"version": version, "say": say});
        self.srv.publish(&mock_bundle_with(
            &self.sk,
            "k1",
            version,
            cfg,
            "The quick brown fox.",
            entry,
        ));
    }
    pub async fn install(&self, version: &str, say: &str) {
        self.publish(version, say);
        let health = ModuleHealth {
            opts: self.opts.clone(),
        };
        let now = wall;
        let r = Updater {
            store: &self.store,
            dl: &Downloader::new(&self.srv.base),
            health: &health,
            now_ms: &now,
        }
        .install_latest(false, &DownloadControl::default(), &|_| {})
        .await
        .unwrap();
        assert!(matches!(r, UpdateOutcome::Installed { .. }), "{r:?}");
    }
}
