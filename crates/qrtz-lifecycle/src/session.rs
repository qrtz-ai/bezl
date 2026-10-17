use qrtz_model::{Degradation, LifecycleEvent, ReconnectInfo, RecordingPhase, RecordingState};

/// `initialRecordingState()`.
pub fn initial_state() -> RecordingState {
    RecordingState::default()
}

/// Phases in which a capture is live (the helper owns an open WAV).
pub const CAPTURING: [RecordingPhase; 3] = [
    RecordingPhase::Starting,
    RecordingPhase::Recording,
    RecordingPhase::Reconnecting,
];

pub fn is_capturing(p: RecordingPhase) -> bool {
    CAPTURING.contains(&p)
}

/// Phases in which a new recording may begin (post-processing runs per meeting, off the session).
pub fn can_start(p: RecordingPhase) -> bool {
    matches!(
        p,
        RecordingPhase::Idle
            | RecordingPhase::Detected
            | RecordingPhase::Processing
            | RecordingPhase::Done
            | RecordingPhase::Error
    )
}

/// `lifecycleReducer`: the next state, or `None` when the event is not legal in this state (callers log and
/// ignore).
pub fn reduce(s: &RecordingState, e: &LifecycleEvent) -> Option<RecordingState> {
    use LifecycleEvent as E;
    use RecordingPhase as P;
    match e {
        E::Detected { meeting } => {
            // Allowed while the last meeting post-processes (back-to-back meetings); its pipeline runs on.
            if !can_start(s.phase) {
                return None;
            }
            Some(RecordingState {
                phase: P::Detected,
                detected: Some(meeting.clone()),
                ..initial_state()
            })
        }
        E::MeetingEnded { key } => {
            if s.phase != P::Detected || s.detected.as_ref().map(|d| &d.key) != Some(key) {
                return None;
            }
            Some(initial_state())
        }
        E::StartRequested {
            meeting_id,
            at,
            audio_path,
        } => {
            if !can_start(s.phase) {
                return None;
            }
            Some(RecordingState {
                phase: P::Starting,
                detected: s.detected.clone(),
                meeting_id: Some(meeting_id.clone()),
                started_at: Some(*at),
                audio_parts: vec![audio_path.clone()],
                ..initial_state()
            })
        }
        E::StartAcked => {
            if s.phase != P::Starting {
                return None;
            }
            Some(RecordingState {
                phase: P::Recording,
                ..s.clone()
            })
        }
        E::StartFailed { error } => {
            if s.phase != P::Starting {
                return None;
            }
            Some(RecordingState {
                phase: if s.detected.is_some() {
                    P::Detected
                } else {
                    P::Idle
                },
                detected: s.detected.clone(),
                error: Some(error.clone()),
                ..initial_state()
            })
        }
        E::Degraded { flag } => {
            if !is_capturing(s.phase) || s.degraded.contains(flag) {
                return None;
            }
            let mut n = s.clone();
            n.degraded.push(*flag);
            Some(n)
        }
        E::HelperLost { at, deadline_at } => {
            if s.phase != P::Recording && s.phase != P::Starting {
                if s.phase != P::Reconnecting {
                    return None;
                }
                // Another loss while reconnecting: count it, keep the original deadline (TS quirk kept).
                let r = s.reconnect.as_ref()?;
                return Some(RecordingState {
                    reconnect: Some(ReconnectInfo {
                        attempt: r.attempt + 1,
                        ..r.clone()
                    }),
                    ..s.clone()
                });
            }
            Some(RecordingState {
                phase: P::Reconnecting,
                reconnect: Some(ReconnectInfo {
                    attempt: 1,
                    since: *at,
                    deadline_at: *deadline_at,
                }),
                ..s.clone()
            })
        }
        E::HelperRestored { at, audio_path } => {
            if s.phase != P::Reconnecting {
                return None;
            }
            let mut n = s.clone();
            n.reconnect = None;
            n.phase = P::Recording;
            n.audio_parts.push(audio_path.clone());
            n.part_offset_ms = at - s.started_at.unwrap_or(*at);
            if !n.degraded.contains(&Degradation::TranscriptGap) {
                n.degraded.push(Degradation::TranscriptGap);
            }
            Some(n)
        }
        E::StopRequested { reason, at } => {
            if !is_capturing(s.phase) {
                return None;
            }
            Some(RecordingState {
                reconnect: None,
                phase: P::Stopping,
                stop_reason: Some(*reason),
                stopped_at: Some(*at),
                ..s.clone()
            })
        }
        E::Stopped { .. } => {
            if s.phase != P::Stopping {
                return None;
            }
            Some(RecordingState {
                phase: P::Processing,
                ..s.clone()
            })
        }
        E::ProcessingDone => {
            if s.phase != P::Processing {
                return None;
            }
            Some(RecordingState {
                phase: P::Done,
                ..s.clone()
            })
        }
        E::ProcessingFailed { error } => {
            if s.phase != P::Processing {
                return None;
            }
            Some(RecordingState {
                phase: P::Error,
                error: Some(error.clone()),
                ..s.clone()
            })
        }
        E::Rejected { error } => {
            // A start refused before anything was created (e.g. permissions): record why, keep the phase.
            if !can_start(s.phase) {
                return None;
            }
            Some(RecordingState {
                error: Some(error.clone()),
                ..s.clone()
            })
        }
        E::ClearError => {
            s.error.as_ref()?;
            Some(RecordingState {
                error: None,
                ..s.clone()
            })
        }
        E::Reset => {
            if is_capturing(s.phase) || s.phase == P::Stopping {
                return None;
            }
            Some(initial_state())
        }
    }
}

/// `LifecycleMachine`: holds the state and applies legal events. Listeners live in the runtime.
#[derive(Debug, Clone, Default)]
pub struct Machine {
    state: RecordingState,
}

impl Machine {
    pub fn new(initial: RecordingState) -> Machine {
        Machine { state: initial }
    }

    pub fn state(&self) -> &RecordingState {
        &self.state
    }

    /// Applies `e` if legal and returns the new state; `None` (state unchanged) otherwise.
    pub fn send(&mut self, e: &LifecycleEvent) -> Option<&RecordingState> {
        let next = reduce(&self.state, e)?;
        self.state = next;
        Some(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_ignores_illegal_events_and_keeps_state() {
        let mut m = Machine::default();
        assert!(m.send(&LifecycleEvent::StartAcked).is_none());
        assert_eq!(m.state(), &initial_state());
        let at = 7;
        let s = m
            .send(&LifecycleEvent::StartRequested {
                meeting_id: "m".into(),
                at,
                audio_path: "/a".into(),
            })
            .unwrap();
        assert_eq!(s.phase, RecordingPhase::Starting);
        assert!(is_capturing(s.phase) && !can_start(s.phase));
    }
}
