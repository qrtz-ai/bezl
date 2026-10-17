/**
 * N35 bounds (also crates/qrtz-lifecycle/tests/n35.rs): a helper that reconnects and fails its mic device on every
 * part keeps at most CAPTURE_LIMITS.deviceFailures records, each reason cut on code points, and every reconnect
 * clears the device-only flag.
 */
import { describe, expect, it } from 'vitest';
import {
  CAPTURE_LIMITS,
  captureReport,
  initialRecordingState,
  lifecycleReducer,
  type LifecycleEvent,
  type RecordingState,
} from './lifecycle';

const step = (s: RecordingState, e: LifecycleEvent) =>
  lifecycleReducer(s, e) ?? s;

describe('N35 device failures stay bounded across a reconnect loop', () => {
  it('20 parts, each failing its mic: 8 records, flag cleared by every reconnect', () => {
    const t0 = 1_760_000_000_000;
    let s = step(initialRecordingState(), {
      type: 'start-requested',
      meetingId: 'm',
      at: t0,
      audioPath: '/r/p0.wav',
    });
    s = step(s, { type: 'start-acked' });
    for (let i = 0; i < 20; i++) {
      const at = t0 + i * 10_000;
      s = step(s, {
        type: 'capture-degraded',
        degraded: ['no-mic'],
        reason: '🎙️ busy '.repeat(100),
        at: at + 1000,
      });
      expect(s.degraded).toContain('no-mic');
      s = step(s, {
        type: 'helper-lost',
        at: at + 2000,
        deadlineAt: at + 32000,
      });
      s = step(s, {
        type: 'helper-restored',
        at: at + 3000,
        audioPath: `/r/p${i + 1}.wav`,
      });
      expect(s.degraded).toEqual(['transcript-gap']);
      expect(s.deviceLost).toBeUndefined();
    }
    const r = captureReport(s);
    expect(r?.deviceFailures).toHaveLength(CAPTURE_LIMITS.deviceFailures);
    expect(r?.deviceFailures?.[7]?.atMs).toBe(71_000);
    for (const d of r?.deviceFailures ?? [])
      expect(Array.from(d.reason)).toHaveLength(CAPTURE_LIMITS.reasonChars);
    // N39: each failure is a lost stretch that its reconnect ends; kept up to the cap.
    expect(r?.lostSpans).toHaveLength(CAPTURE_LIMITS.lostSpans);
    for (const [i, x] of (r?.lostSpans ?? []).entries())
      expect(x).toEqual({
        source: 'mic',
        startMs: i * 10_000 + 1000,
        endMs: i * 10_000 + 3000,
      });
  });
});
