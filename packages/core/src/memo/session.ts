import type { MeetingPlatform } from './types';

export type SessionPhase =
  | 'idle'
  | 'detected'
  | 'recording'
  | 'processing'
  | 'done'
  | 'error';

export interface DetectedMeeting {
  key: string;
  title: string;
  platform: MeetingPlatform;
  app: string;
}

export interface SessionState {
  phase: SessionPhase;
  detected?: DetectedMeeting;
  meetingId?: string;
  startedAt?: number;
  stoppedAt?: number;
  error?: string;
}

export type SessionEvent =
  | { type: 'detected'; meeting: DetectedMeeting }
  | { type: 'meeting-ended'; key: string }
  | { type: 'start'; meetingId: string; at: number }
  | { type: 'stop'; at: number }
  | { type: 'processed' }
  | { type: 'failed'; error: string }
  | { type: 'reset' };

/**
 * Pure meeting-session reducer: idle → detected → recording → processing → done.
 * Returns `null` for transitions that aren't allowed so callers can log and ignore.
 */
export function sessionReducer(
  s: SessionState,
  e: SessionEvent,
): SessionState | null {
  switch (e.type) {
    case 'detected':
      if (s.phase === 'recording' || s.phase === 'processing') return null;
      return { phase: 'detected', detected: e.meeting };
    case 'meeting-ended':
      if (s.detected?.key !== e.key) return null;
      if (s.phase === 'detected') return { phase: 'idle' };
      return null; // recording: host decides to stop (it emits 'stop')
    case 'start':
      if (s.phase === 'recording' || s.phase === 'processing') return null;
      return {
        phase: 'recording',
        ...(s.detected ? { detected: s.detected } : {}),
        meetingId: e.meetingId,
        startedAt: e.at,
      };
    case 'stop':
      if (s.phase !== 'recording') return null;
      return { ...s, phase: 'processing', stoppedAt: e.at };
    case 'processed':
      if (s.phase !== 'processing') return null;
      return { ...s, phase: 'done' };
    case 'failed':
      return { ...s, phase: 'error', error: e.error };
    case 'reset':
      if (s.phase === 'recording' || s.phase === 'processing') return null;
      return { phase: 'idle' };
  }
}

export class SessionMachine {
  private s: SessionState = { phase: 'idle' };
  private listeners = new Set<(s: SessionState) => void>();
  get state(): SessionState {
    return this.s;
  }
  send(e: SessionEvent): boolean {
    const next = sessionReducer(this.s, e);
    if (!next) return false;
    this.s = next;
    for (const l of this.listeners) l(next);
    return true;
  }
  onChange(cb: (s: SessionState) => void) {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }
}
