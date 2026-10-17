import { useCallback, useEffect, useState } from 'react';
import {
  SessionView,
  applyTranscriptDelta,
  type LiveTranscript,
  type PipelineView,
  type StepId,
} from '@qrtz/core';
import { useQrtz } from '@qrtz/core/react';

/**
 * Renderer state for the recording lifecycle. Everything is derived from main over typed IPC: snapshot on
 * mount, then events. No local guesses: if a stream delta is missing, the snapshot is refetched.
 */

export function useSession(): SessionView {
  const qrtz = useQrtz();
  const [s, setS] = useState<SessionView>(() =>
    SessionView.parse({ phase: 'idle' }),
  );
  useEffect(() => {
    let alive = true;
    const off = qrtz.on('note:session', setS);
    void qrtz.invoke('note:session').then(
      (v) => alive && setS(v),
      () => {},
    );
    return () => {
      alive = false;
      off();
    };
  }, [qrtz]);
  return s;
}

/** Post-processing state of one meeting, live. */
export function usePipeline(meetingId: string | null | undefined) {
  const qrtz = useQrtz();
  const [p, setP] = useState<PipelineView | null>(null);
  useEffect(() => {
    setP(null);
    if (!meetingId) return;
    let alive = true;
    const off = qrtz.on('note:pipeline', (v) => {
      if (v.meetingId === meetingId) setP(v);
    });
    void qrtz.invoke('note:pipeline', { id: meetingId }).then(
      (v) => alive && v && setP(v),
      () => {},
    );
    return () => {
      alive = false;
      off();
    };
  }, [qrtz, meetingId]);
  const retry = useCallback(
    (step: StepId) => {
      if (meetingId)
        void qrtz
          .invoke('note:retry-step', { id: meetingId, step })
          .then(setP, () => {});
    },
    [qrtz, meetingId],
  );
  return { pipeline: p, retry };
}

/** Live transcript of the meeting being recorded (null when `meetingId` is not the live one). */
export function useLiveTranscript(
  meetingId: string | null | undefined,
): LiveTranscript | null {
  const qrtz = useQrtz();
  const [t, setT] = useState<LiveTranscript | null>(null);
  useEffect(() => {
    setT(null);
    if (!meetingId) return;
    let alive = true;
    let cur: LiveTranscript | null = null;
    let fetching = false;
    const refetch = () => {
      if (fetching) return;
      fetching = true;
      void qrtz
        .invoke('note:live-transcript', { meetingId })
        .then(
          (snap) => {
            // Keep whichever is newer: deltas may have advanced past the snapshot while it was in flight.
            if (alive && snap && (!cur || snap.rev >= cur.rev))
              setT((cur = snap));
          },
          () => {},
        )
        .finally(() => (fetching = false));
    };
    const off = qrtz.on('note:transcript', (d) => {
      if (d.meetingId !== meetingId) return;
      const next = applyTranscriptDelta(cur, d);
      if (next) setT((cur = next));
      else if (!cur || d.rev > cur.rev) refetch(); // gap: resync from the source of truth
    });
    refetch();
    return () => {
      alive = false;
      off();
    };
  }, [qrtz, meetingId]);
  return t;
}

export const STEP_LABELS: Record<StepId, string> = {
  finalize: 'Transcript',
  summary: 'Summary',
  index: 'Search index',
};

export const DEGRADED_LABELS: Record<SessionView['degraded'][number], string> =
  {
    'asr-unavailable': 'No live transcription',
    'audio-unavailable': 'No audio',
    'mic-only': 'Mic only',
    'no-mic': 'No mic',
    'ax-unavailable': 'No speaker names',
    'transcript-gap': 'Gap after reconnect',
  };

/** N29/N32: live "no sound" chip labels, per capture source the helper reported silent, and for all of them. */
export const SILENT_LABELS: Record<
  NonNullable<SessionView['silent']>[number]['source'] | 'all',
  string
> = {
  system: 'No sound from system audio',
  mic: 'No sound from mic',
  all: 'No sound',
};

/**
 * N32: the capture-health chips of the live view. The sticky degradations, then the sources that are silent right
 * now (N29): one "No sound" chip when every source still delivered is silent (`noAudio`), else one per silent
 * source. Silence is live: the chip goes away when the helper reports the source audible again.
 */
export function captureChips(
  s: Pick<SessionView, 'degraded' | 'silent' | 'noAudio'>,
): { id: string; label: string }[] {
  const chips: { id: string; label: string }[] = s.degraded.map((f) => ({
    id: f,
    label: DEGRADED_LABELS[f],
  }));
  // `audio-unavailable` already says there is no audio at all.
  if (s.degraded.includes('audio-unavailable')) return chips;
  if (s.noAudio)
    return [...chips, { id: 'no-sound', label: SILENT_LABELS.all }];
  for (const x of s.silent ?? [])
    chips.push({ id: `silent-${x.source}`, label: SILENT_LABELS[x.source] });
  return chips;
}

export const isCapturingPhase = (p: SessionView['phase']) =>
  p === 'starting' || p === 'recording' || p === 'reconnecting';
