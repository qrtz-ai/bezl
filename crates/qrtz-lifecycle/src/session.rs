use qrtz_model::{
    CaptureFallback, CaptureReport, CaptureSource, Degradation, DeviceFailure, EpochMs, LifecycleEvent, Ms,
    ReconnectInfo, RecordingPhase, RecordingState, SilentInput, SilentSpan,
};

/// Bounds on the capture record (`CAPTURE_LIMITS` in lifecycle.ts).
pub mod capture_limits {
    pub const SILENT_SPANS: usize = 256;
    pub const FALLBACKS: usize = 8;
    /// N35: `capture.degraded` records kept per recording.
    pub const DEVICE_FAILURES: usize = 8;
    /// N39: lost-source stretches kept per recording.
    pub const LOST_SPANS: usize = 16;
    /// `capture.fallback` strings are kept to this many code points.
    pub const REASON_CHARS: usize = 200;
}

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

/// Sources a degradation takes away: their silence means nothing any more (N29; `SOURCES_LOST`).
fn sources_lost(flag: Degradation) -> &'static [CaptureSource] {
    match flag {
        Degradation::MicOnly => &[CaptureSource::System],
        Degradation::NoMic => &[CaptureSource::Mic],
        Degradation::AudioUnavailable => &[CaptureSource::System, CaptureSource::Mic],
        _ => &[],
    }
}

/// `sourceLost`: `degraded[]` says the helper no longer delivers `src`.
pub fn source_lost(degraded: &[Degradation], src: CaptureSource) -> bool {
    degraded.iter().any(|d| sources_lost(*d).contains(&src))
}

/// Meeting-time ms of a wall-clock instant, never before `floor`.
fn meeting_ms(s: &RecordingState, at: EpochMs, floor: Ms) -> Ms {
    (at - s.started_at.unwrap_or(at)).max(floor)
}

/// `closeSilent`: the silent stretches of `sources` (all when `None`) end at `at` (zero-length when `at` is `None`).
fn close_silent(
    mut s: RecordingState,
    at: Option<EpochMs>,
    sources: Option<&[CaptureSource]>,
) -> RecordingState {
    let hit = |x: &SilentInput| sources.is_none_or(|l| l.contains(&x.source));
    let (closing, open): (Vec<SilentInput>, Vec<SilentInput>) =
        std::mem::take(&mut s.silent).into_iter().partition(hit);
    s.silent = open;
    for x in closing {
        if s.silent_spans.len() < capture_limits::SILENT_SPANS {
            let end = at.map_or(x.since_ms, |at| meeting_ms(&s, at, x.since_ms));
            s.silent_spans.push(SilentSpan {
                source: x.source,
                start_ms: x.since_ms,
                end_ms: Some(end),
            });
        }
    }
    s
}

/// `openLost` (N39): a lost stretch opens at `at` for each of `sources` with none open (capped).
fn open_lost(mut s: RecordingState, at: EpochMs, sources: &[CaptureSource]) -> RecordingState {
    for src in sources {
        if !s
            .lost_spans
            .iter()
            .any(|x| x.source == *src && x.end_ms.is_none())
            && s.lost_spans.len() < capture_limits::LOST_SPANS
        {
            let start = meeting_ms(&s, at, 0);
            s.lost_spans.push(SilentSpan {
                source: *src,
                start_ms: start,
                end_ms: None,
            });
        }
    }
    s
}

/// `closeLost` (N39): open lost stretches of sources `degraded[]` no longer takes away (all when `all`) end at `at`.
fn close_lost(mut s: RecordingState, at: EpochMs, all: bool) -> RecordingState {
    let degraded = s.degraded.clone();
    let started = s.started_at;
    for x in s.lost_spans.iter_mut() {
        if x.end_ms.is_none() && (all || !source_lost(&degraded, x.source)) {
            x.end_ms = Some((at - started.unwrap_or(at)).max(x.start_ms));
        }
    }
    s
}

/// N42 (ADR 0034, `HELPER_SCOPED`): flags a helper reports at each recording start, before `recording.started`. They
/// die with that helper; the next one reports its own while the session is reconnecting.
const HELPER_SCOPED: [Degradation; 2] = [Degradation::AsrUnavailable, Degradation::AudioUnavailable];

/// `dropHelperScoped` (N42): drops the helper-scoped flags at a helper loss; a source they took away is no longer lost.
fn drop_helper_scoped(mut s: RecordingState, at: EpochMs) -> RecordingState {
    if !s.degraded.iter().any(|f| HELPER_SCOPED.contains(f)) {
        return s;
    }
    s.degraded.retain(|f| !HELPER_SCOPED.contains(f));
    s.device_lost.retain(|f| !HELPER_SCOPED.contains(f));
    close_lost(s, at, false)
}

/// `cutChars`: a code-point cut (TS `Array.from(v).slice(0, n)`).
pub fn cut_chars(v: &str, n: usize) -> String {
    v.chars().take(n).collect()
}

/// `captureReport`: what a stopped recording hands to its pipeline; stretches still open keep no end.
pub fn capture_report(s: &RecordingState) -> Option<CaptureReport> {
    let mut spans = s.silent_spans.clone();
    spans.extend(s.silent.iter().map(|x| SilentSpan {
        source: x.source,
        start_ms: x.since_ms,
        end_ms: None,
    }));
    spans.truncate(capture_limits::SILENT_SPANS);
    if spans.is_empty() && s.fallbacks.is_empty() && s.device_failures.is_empty() && s.lost_spans.is_empty() {
        return None;
    }
    Some(CaptureReport {
        silent_spans: spans,
        fallbacks: s.fallbacks.clone(),
        device_failures: s.device_failures.clone(),
        lost_spans: s.lost_spans.clone(),
    })
}

/// `silentInputView`: the live `silent` list and `noAudio` (every source still delivered is silent). A source counts
/// as delivered unless `degraded[]` took it away or its permission is denied.
pub fn silent_input_view(
    s: &RecordingState,
    mic_denied: bool,
    screen_denied: bool,
) -> (Vec<SilentInput>, bool) {
    if !is_capturing(s.phase) || s.silent.is_empty() {
        return (Vec::new(), false);
    }
    let delivered: Vec<CaptureSource> = [CaptureSource::System, CaptureSource::Mic]
        .into_iter()
        .filter(|src| {
            !source_lost(&s.degraded, *src)
                && !match src {
                    CaptureSource::Mic => mic_denied,
                    CaptureSource::System => screen_denied,
                }
        })
        .collect();
    let all = !delivered.is_empty()
        && delivered
            .iter()
            .all(|src| s.silent.iter().any(|x| x.source == *src));
    (s.silent.clone(), all)
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
        E::Degraded { flag, at } => {
            if !is_capturing(s.phase) {
                return None;
            }
            if s.degraded.contains(flag) {
                // N35: a revoke of a flag a device failure set: the permission owns it now (it outlives a reconnect).
                if !s.device_lost.contains(flag) {
                    return None;
                }
                let mut n = s.clone();
                n.device_lost.retain(|f| f != flag);
                return Some(n);
            }
            let mut n = s.clone();
            n.degraded.push(*flag);
            // A source this flag takes away is no longer tracked by the helper: close its silent stretch (N29), and
            // it is lost from now on (N39; an event without a time records no lost stretch).
            let lost = sources_lost(*flag);
            if lost.is_empty() {
                return Some(n);
            }
            let closed = close_silent(n, *at, Some(lost));
            Some(match at {
                Some(at) => open_lost(closed, *at, lost),
                None => closed,
            })
        }
        E::InputSilent { source, since_ms } => {
            if !matches!(s.phase, P::Starting | P::Recording)
                || s.silent.iter().any(|x| x.source == *source)
                || source_lost(&s.degraded, *source)
            {
                return None;
            }
            let mut n = s.clone();
            n.silent.push(SilentInput {
                source: *source,
                since_ms: (*since_ms + s.part_offset_ms).max(0),
            });
            Some(n)
        }
        E::InputAudible { source, at } => {
            if !matches!(s.phase, P::Starting | P::Recording) || !s.silent.iter().any(|x| x.source == *source)
            {
                return None;
            }
            Some(close_silent(s.clone(), Some(*at), Some(&[*source])))
        }
        E::CaptureDegraded { degraded, reason, at } => {
            // N35: applied like a revoke (N4), remembered as a device failure; only flags not already set count.
            // Only while recording: the helper sends it after recording.started, so one before the ack is stray.
            if s.phase != P::Recording {
                return None;
            }
            let mut fresh: Vec<Degradation> = Vec::new();
            for f in degraded {
                if !s.degraded.contains(f) && !fresh.contains(f) {
                    fresh.push(*f);
                }
            }
            if fresh.is_empty() {
                return None;
            }
            let mut n = s.clone();
            n.degraded.extend(fresh.iter().copied());
            n.device_lost.extend(fresh.iter().copied());
            if n.device_failures.len() < capture_limits::DEVICE_FAILURES {
                n.device_failures.push(DeviceFailure {
                    degraded: fresh.clone(),
                    reason: cut_chars(reason, capture_limits::REASON_CHARS),
                    at_ms: meeting_ms(s, *at, 0),
                });
            }
            let mut lost: Vec<CaptureSource> = Vec::new();
            for src in fresh.iter().flat_map(|f| sources_lost(*f)) {
                if !lost.contains(src) {
                    lost.push(*src);
                }
            }
            Some(if lost.is_empty() {
                n
            } else {
                open_lost(close_silent(n, Some(*at), Some(&lost)), *at, &lost)
            })
        }
        E::CaptureFallback { from, to, reason, at } => {
            if !is_capturing(s.phase) {
                return None;
            }
            let mut n = s.clone();
            let mut changed = false;
            if n.fallbacks.len() < capture_limits::FALLBACKS {
                n.fallbacks.push(CaptureFallback {
                    from: cut_chars(from, capture_limits::REASON_CHARS),
                    to: to.as_deref().map(|t| cut_chars(t, capture_limits::REASON_CHARS)),
                    reason: cut_chars(reason, capture_limits::REASON_CHARS),
                    at_ms: meeting_ms(s, *at, 0),
                });
                changed = true;
            }
            // No source left: nothing is captured (`error{audio.unavailable}` follows and is then a no-op).
            if to.is_none() && !n.degraded.contains(&Degradation::AudioUnavailable) {
                n.degraded.push(Degradation::AudioUnavailable);
                n = open_lost(
                    close_silent(n, Some(*at), None),
                    *at,
                    &[CaptureSource::System, CaptureSource::Mic],
                );
                changed = true;
            }
            changed.then_some(n)
        }
        E::HelperLost { at, deadline_at } => {
            if s.phase != P::Recording && s.phase != P::Starting {
                if s.phase != P::Reconnecting {
                    return None;
                }
                // Another loss while reconnecting: count it, keep the original deadline (TS quirk kept). N42: a
                // helper that died while reconnecting takes its own start reports with it, too.
                let r = s.reconnect.as_ref()?;
                return Some(RecordingState {
                    reconnect: Some(ReconnectInfo {
                        attempt: r.attempt + 1,
                        ..r.clone()
                    }),
                    ..drop_helper_scoped(s.clone(), *at)
                });
            }
            // The helper's silence trackers die with it: close every silent stretch at the loss (N29). N42: so do
            // its start reports.
            Some(RecordingState {
                phase: P::Reconnecting,
                reconnect: Some(ReconnectInfo {
                    attempt: 1,
                    since: *at,
                    deadline_at: *deadline_at,
                }),
                ..drop_helper_scoped(close_silent(s.clone(), Some(*at), None), *at)
            })
        }
        E::HelperRestored { at, audio_path } => {
            if s.phase != P::Reconnecting {
                return None;
            }
            let mut n = s.clone();
            n.reconnect = None;
            n.phase = P::Recording;
            // Clamp: a backward clock jump must not invent negative meeting-time offsets.
            // N45 (ADR 0037): the runtime stamps `at` from a monotonic clock for helper-lost/restored.
            n.part_offset_ms = (*at - s.started_at.unwrap_or(*at)).max(0);
            // H7: the offsets are kept only while they line up with the parts (a row from an older build has none).
            n.part_offsets_ms = match &s.part_offsets_ms {
                Some(v) if v.len() == s.audio_parts.len() => Some(v.clone()),
                None if s.audio_parts.len() == 1 => Some(vec![0]),
                _ => None,
            }
            .map(|mut v| {
                v.push(n.part_offset_ms);
                v
            });
            n.audio_parts.push(audio_path.clone());
            // N35: a device failure lasts until the reconnect: the new helper starts from its own state.
            let lost = std::mem::take(&mut n.device_lost);
            n.degraded.retain(|f| !lost.contains(f));
            if !n.degraded.contains(&Degradation::TranscriptGap) {
                n.degraded.push(Degradation::TranscriptGap);
            }
            // N39: a source the reconnect brings back is no longer lost.
            Some(close_lost(n, *at, false))
        }
        E::StopRequested { reason, at } => {
            if !is_capturing(s.phase) {
                return None;
            }
            // A source still silent at the stop: its stretch ends at the stop (N29).
            Some(RecordingState {
                reconnect: None,
                phase: P::Stopping,
                stop_reason: Some(*reason),
                stopped_at: Some(*at),
                ..close_lost(close_silent(s.clone(), Some(*at), None), *at, true)
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
            // From Error: a manual retry of the failed step succeeded; the error is resolved.
            if s.phase != P::Processing && s.phase != P::Error {
                return None;
            }
            Some(RecordingState {
                phase: P::Done,
                error: None,
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
        E::SpeechReady => {
            // N42 (ADR 0046 §3b): speech-unavailable is scoped to the speech session that reported it.
            if s.phase != P::Recording || !s.degraded.contains(&Degradation::AsrUnavailable) {
                return None;
            }
            let mut n = s.clone();
            n.degraded.retain(|f| *f != Degradation::AsrUnavailable);
            n.device_lost.retain(|f| *f != Degradation::AsrUnavailable);
            Some(n)
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

    #[test]
    fn helper_restore_with_backward_clock_clamps_part_offset() {
        let mut m = Machine::default();
        m.send(&LifecycleEvent::StartRequested {
            meeting_id: "m".into(),
            at: 10_000,
            audio_path: "/p0".into(),
        })
        .unwrap();
        m.send(&LifecycleEvent::StartAcked).unwrap();
        m.send(&LifecycleEvent::HelperLost {
            at: 12_000,
            deadline_at: 42_000,
        })
        .unwrap();
        let s = m
            .send(&LifecycleEvent::HelperRestored {
                at: 9_000,
                audio_path: "/p1".into(),
            })
            .unwrap();
        assert_eq!(s.part_offset_ms, 0);
        assert!(s.degraded.contains(&Degradation::TranscriptGap));
        assert_eq!(s.audio_parts, vec!["/p0".to_string(), "/p1".to_string()]);
    }
}
