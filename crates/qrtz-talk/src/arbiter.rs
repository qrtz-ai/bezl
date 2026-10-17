//! `EngineArbiter` (packages/core/src/talk/asr.ts) as a pure state machine: which engine's text to show and
//! insert. The on-device stream (the helper's `dictation.asr`) always runs; an opt-in cloud stream wins while it
//! is healthy and fails over to on-device on a socket error, when it stays silent while on-device already has
//! words (`stall_ms`), or when its final is late (the caller passes that as an error). Time is passed in (mono
//! ms), so the same steps give the same answers in TS and Rust (spec/vectors/talk/engine-arbiter.json).
use crate::contract::Engine;
use crate::provider::join;

#[derive(Debug, Clone, PartialEq)]
pub struct ArbiterResult {
    pub text: String,
    pub engine: Engine,
    pub failover: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Arbiter {
    cloud: Option<Engine>,
    cloud_committed: String,
    cloud_partial: String,
    local_finals: Vec<String>,
    local_partial: String,
    cloud_failed: Option<String>,
    cloud_heard: bool,
    local_since: Option<f64>,
    stall_ms: f64,
}

impl Arbiter {
    /// `stall_ms = f64::INFINITY` disables the stall check (auto-failover off).
    pub fn new(stall_ms: f64) -> Self {
        Self {
            cloud: None,
            cloud_committed: String::new(),
            cloud_partial: String::new(),
            local_finals: vec![],
            local_partial: String::new(),
            cloud_failed: None,
            cloud_heard: false,
            local_since: None,
            stall_ms,
        }
    }
    /// The socket connects while the mic already streams on-device; attach it once it exists.
    pub fn attach_cloud(&mut self, engine: Engine) {
        self.cloud = Some(engine);
    }
    pub fn cloud_update(&mut self, committed: &str, partial: &str) {
        self.cloud_heard = true;
        self.cloud_committed = committed.to_owned();
        self.cloud_partial = partial.to_owned();
    }
    /// First error wins. Returns true when this call failed the cloud.
    pub fn cloud_error(&mut self, err: &str) -> bool {
        if self.cloud_failed.is_none() {
            self.cloud_failed = Some(err.to_owned());
            return self.cloud.is_some();
        }
        false
    }
    pub fn local_partial(&mut self, text: &str, now: f64) -> bool {
        self.local_partial = text.to_owned();
        self.local_since.get_or_insert(now);
        self.check(now)
    }
    pub fn local_final(&mut self, text: &str, now: f64) -> bool {
        self.local_finals.push(text.to_owned());
        self.local_partial.clear();
        self.local_since.get_or_insert(now);
        self.check(now)
    }
    /// Detects a silent cloud stream. Returns true when this call failed it.
    pub fn check(&mut self, now: f64) -> bool {
        if self.cloud.is_some()
            && self.cloud_failed.is_none()
            && !self.cloud_heard
            && self.local_since.is_some_and(|t| now - t >= self.stall_ms)
        {
            self.cloud_failed = Some("cloud stream stalled".into());
            return true;
        }
        false
    }
    pub fn using_cloud(&self) -> bool {
        self.cloud.is_some() && self.cloud_failed.is_none()
    }
    pub fn failover(&self) -> bool {
        self.cloud.is_some() && self.cloud_failed.is_some()
    }
    pub fn fail_reason(&self) -> Option<&str> {
        self.cloud_failed.as_deref()
    }
    pub fn cloud_engine(&self) -> Option<Engine> {
        self.cloud
    }
    pub fn engine(&self) -> Engine {
        match self.cloud {
            Some(e) if self.cloud_failed.is_none() => e,
            _ => Engine::OnDevice,
        }
    }
    fn local_text(&self) -> (String, String) {
        (
            self.local_finals.join(" ").trim().to_owned(),
            self.local_partial.clone(),
        )
    }
    /// (committed, partial) of the engine in use.
    pub fn view(&self) -> (String, String) {
        if self.using_cloud() {
            (self.cloud_committed.clone(), self.cloud_partial.clone())
        } else {
            self.local_text()
        }
    }
    /// The on-device live text (what the module heard so far).
    pub fn local_live(&self) -> String {
        let (c, p) = self.local_text();
        join(&c, &p)
    }
    /// The cloud's live text, whatever its state (what was heard when it went away).
    pub fn cloud_live(&self) -> String {
        join(&self.cloud_committed, &self.cloud_partial)
    }
    /// After the helper's `dictation.stopped` (`local_final_text` = its full on-device text). `cloud` is the
    /// cloud flush result when the cloud was still in use (`Err` for an error or a late final), else `None`.
    pub fn finish(&mut self, local_final_text: &str, cloud: Option<Result<String, String>>) -> ArbiterResult {
        let (lc, lp) = self.local_text();
        let local = match local_final_text.trim() {
            "" => join(&lc, &lp),
            t => t.to_owned(),
        };
        if self.using_cloud() {
            match cloud {
                Some(Ok(t)) => {
                    let text = match t.trim() {
                        "" => self.cloud_live(),
                        t => t.to_owned(),
                    };
                    // An empty cloud result while on-device heard words is a failure, not silence.
                    if !text.is_empty() || local.is_empty() {
                        return ArbiterResult {
                            text,
                            engine: self.engine(),
                            failover: false,
                            reason: None,
                        };
                    }
                    self.cloud_failed = Some("cloud returned no text".into());
                }
                Some(Err(e)) => {
                    self.cloud_failed.get_or_insert(e);
                }
                None => {
                    self.cloud_failed.get_or_insert("cloud failed".into());
                }
            }
        }
        ArbiterResult {
            text: local,
            engine: Engine::OnDevice,
            failover: self.cloud.is_some(),
            reason: self.cloud_failed.clone(),
        }
    }
}
