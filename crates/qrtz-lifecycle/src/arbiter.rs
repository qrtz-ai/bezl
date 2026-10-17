//! Helper session arbiter, ported from `packages/core/src/helper/arbiter.ts` (Swift twin:
//! `native/qrtz-helper/Sources/QrtzHelperCore/Arbiter.swift`). The rules for running a meeting recording and a
//! dictation side by side on one helper (shared, refcounted mic tap; one instance per kind), the warm mic, and
//! what a mid-capture TCC revocation does to each (docs/helper-protocol.md, ADR 0004).
//!
//! Pure and deterministic. The Tauri host needs [`degradations_for_revocation`] to degrade a live recording on
//! `permission.revoked` exactly like Electron's NoteService, and the full step so a Rust host can predict the
//! helper's answers. Pinned by `spec/vectors/helper/{arbiter,arbiter-edges}.json`, the same files vitest and the
//! Swift tests run (ADR 0006).

use qrtz_model::{Degradation, OsPermission};
use serde::{Deserialize, Serialize};

/// Who reads the shared mic tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MicSubscriber {
    Recording,
    Dictation,
}

impl MicSubscriber {
    fn as_str(self) -> &'static str {
        match self {
            MicSubscriber::Recording => "recording",
            MicSubscriber::Dictation => "dictation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ArbiterState {
    pub recording: bool,
    pub dictating: bool,
    /// Warm mic: the dictation reader stays subscribed between dictations. Survives a mic revoke.
    pub warm: bool,
    /// The hub's reader list. The tap runs while non-empty.
    pub mic: Vec<MicSubscriber>,
    /// Revoked/denied permissions; anything not listed is treated as usable.
    pub denied: Vec<OsPermission>,
}

impl ArbiterState {
    pub fn initial(denied: Vec<OsPermission>) -> Self {
        ArbiterState {
            denied,
            ..Default::default()
        }
    }
    fn reads(&self, who: MicSubscriber) -> bool {
        self.mic.contains(&who)
    }
    fn without(&self, who: MicSubscriber) -> Vec<MicSubscriber> {
        self.mic.iter().copied().filter(|m| *m != who).collect()
    }
    fn with(&self, who: MicSubscriber) -> Vec<MicSubscriber> {
        let mut m = self.mic.clone();
        m.push(who);
        m
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArbiterCmd {
    #[serde(rename = "recording.start")]
    RecordingStart,
    #[serde(rename = "recording.stop")]
    RecordingStop,
    #[serde(rename = "dictation.start")]
    DictationStart,
    #[serde(rename = "dictation.stop")]
    DictationStop,
    #[serde(rename = "dictation.cancel")]
    DictationCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Configure {
    #[serde(rename = "warmMic")]
    pub warm_mic: bool,
}

/// Wire shape matches the TS union: `{cmd}`, `{configure:{warmMic}}`, `{revoked}`, `{granted}`, `{helperLost:true}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArbiterEvent {
    Cmd {
        cmd: ArbiterCmd,
    },
    Configure {
        configure: Configure,
    },
    Revoked {
        revoked: OsPermission,
    },
    Granted {
        granted: OsPermission,
    },
    HelperLost {
        #[serde(rename = "helperLost")]
        helper_lost: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Accept,
    Ignore,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArbiterResult {
    pub state: ArbiterState,
    pub decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Recording and dictation both read the tap after this step.
    pub mux: bool,
    /// `mic.subscribe:<who>`, `mic.unsubscribe:<who>`, `dictation.stop:<reason>`, `recording.lost`,
    /// `degrade:<flag>` (strings so vectors stay language-neutral).
    pub effects: Vec<String>,
}

/// Recording degradations caused by losing `p`, given what was already denied.
pub fn degradations_for_revocation(p: OsPermission, already_denied: &[OsPermission]) -> Vec<Degradation> {
    let has = |x: OsPermission| !already_denied.contains(&x) && x != p;
    vec![match p {
        OsPermission::Accessibility => Degradation::AxUnavailable,
        OsPermission::Microphone if has(OsPermission::ScreenRecording) => Degradation::NoMic,
        OsPermission::ScreenRecording if has(OsPermission::Microphone) => Degradation::MicOnly,
        _ => Degradation::AudioUnavailable,
    }]
}

fn res(state: ArbiterState, decision: Decision, effects: Vec<String>, reason: Option<&str>) -> ArbiterResult {
    let mux = state.reads(MicSubscriber::Recording) && state.reads(MicSubscriber::Dictation);
    ArbiterResult {
        state,
        decision,
        reason: reason.map(str::to_owned),
        mux,
        effects,
    }
}

fn sub(who: MicSubscriber) -> String {
    format!("mic.subscribe:{}", who.as_str())
}
fn unsub(who: MicSubscriber) -> String {
    format!("mic.unsubscribe:{}", who.as_str())
}

pub fn arbiter_step(s: &ArbiterState, e: &ArbiterEvent) -> ArbiterResult {
    use MicSubscriber::{Dictation as D, Recording as R};
    let mic_ok = !s.denied.contains(&OsPermission::Microphone);
    let has_dict = s.reads(D);
    match *e {
        ArbiterEvent::HelperLost { .. } => {
            let mut fx = Vec::new();
            if s.dictating {
                fx.push("dictation.stop:helper-lost".to_owned());
            }
            if s.recording {
                fx.push("recording.lost".to_owned());
            }
            fx.extend(s.mic.iter().map(|w| unsub(*w)));
            res(
                ArbiterState::initial(s.denied.clone()),
                Decision::Accept,
                fx,
                None,
            )
        }
        ArbiterEvent::Configure { configure } => {
            let warm = configure.warm_mic;
            if warm && mic_ok && !has_dict {
                return res(
                    ArbiterState {
                        warm,
                        mic: s.with(D),
                        ..s.clone()
                    },
                    Decision::Accept,
                    vec![sub(D)],
                    None,
                );
            }
            if !warm && has_dict && !s.dictating {
                return res(
                    ArbiterState {
                        warm,
                        mic: s.without(D),
                        ..s.clone()
                    },
                    Decision::Accept,
                    vec![unsub(D)],
                    None,
                );
            }
            res(ArbiterState { warm, ..s.clone() }, Decision::Accept, vec![], None)
        }
        ArbiterEvent::Granted { granted } => {
            let next = ArbiterState {
                denied: s.denied.iter().copied().filter(|p| *p != granted).collect(),
                ..s.clone()
            };
            if granted == OsPermission::Microphone && !mic_ok && s.warm && !has_dict {
                let mic = next.with(D);
                return res(ArbiterState { mic, ..next }, Decision::Accept, vec![sub(D)], None);
            }
            res(next, Decision::Accept, vec![], None)
        }
        ArbiterEvent::Revoked { revoked: p } => {
            if s.denied.contains(&p) {
                return res(s.clone(), Decision::Ignore, vec![], Some("unchanged"));
            }
            let mut fx = Vec::new();
            let mut next = s.clone();
            next.denied.push(p);
            if p == OsPermission::Microphone {
                if s.dictating {
                    fx.push("dictation.stop:permission-revoked".to_owned());
                    next.dictating = false;
                }
                fx.extend(s.mic.iter().map(|w| unsub(*w)));
                next.mic.clear();
            }
            if s.recording {
                fx.extend(
                    degradations_for_revocation(p, &s.denied)
                        .into_iter()
                        .map(|f| format!("degrade:{}", f.as_str())),
                );
            }
            res(next, Decision::Accept, fx, None)
        }
        ArbiterEvent::Cmd { cmd } => match cmd {
            ArbiterCmd::RecordingStart => {
                if s.recording {
                    return res(s.clone(), Decision::Ignore, vec![], Some("already-recording"));
                }
                let subscribe = mic_ok && !s.reads(R);
                let mic = if subscribe { s.with(R) } else { s.mic.clone() };
                let fx = if subscribe { vec![sub(R)] } else { vec![] };
                res(
                    ArbiterState {
                        recording: true,
                        mic,
                        ..s.clone()
                    },
                    Decision::Accept,
                    fx,
                    None,
                )
            }
            ArbiterCmd::RecordingStop => {
                if !s.recording {
                    return res(s.clone(), Decision::Ignore, vec![], Some("not-recording"));
                }
                let fx = if s.reads(R) { vec![unsub(R)] } else { vec![] };
                res(
                    ArbiterState {
                        recording: false,
                        mic: s.without(R),
                        ..s.clone()
                    },
                    Decision::Accept,
                    fx,
                    None,
                )
            }
            ArbiterCmd::DictationStart => {
                if s.dictating {
                    return res(s.clone(), Decision::Ignore, vec![], Some("already-dictating"));
                }
                if !mic_ok {
                    return res(s.clone(), Decision::Refuse, vec![], Some("microphone-denied"));
                }
                let mic = if has_dict { s.mic.clone() } else { s.with(D) };
                let fx = if has_dict { vec![] } else { vec![sub(D)] };
                res(
                    ArbiterState {
                        dictating: true,
                        mic,
                        ..s.clone()
                    },
                    Decision::Accept,
                    fx,
                    None,
                )
            }
            ArbiterCmd::DictationStop | ArbiterCmd::DictationCancel => {
                if !s.dictating {
                    return res(s.clone(), Decision::Ignore, vec![], Some("not-dictating"));
                }
                if s.warm && has_dict {
                    return res(
                        ArbiterState {
                            dictating: false,
                            ..s.clone()
                        },
                        Decision::Accept,
                        vec![],
                        None,
                    );
                }
                let fx = if has_dict { vec![unsub(D)] } else { vec![] };
                res(
                    ArbiterState {
                        dictating: false,
                        mic: s.without(D),
                        ..s.clone()
                    },
                    Decision::Accept,
                    fx,
                    None,
                )
            }
        },
    }
}
