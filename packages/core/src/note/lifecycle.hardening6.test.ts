/**
 * Hardening cycle 6 stress (spec/stress/capture-bounds.json, also run by crates/qrtz-lifecycle/tests/hardening6.rs):
 * hours of a flapping capture source and a fallback loop with huge error text keep the session row and the capture
 * report bounded (N29/H8 limits), and every cut is code-point safe.
 */
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import {
  CAPTURE_LIMITS,
  captureReport,
  initialRecordingState,
  lifecycleReducer,
  type LifecycleEvent,
  type RecordingState,
} from './lifecycle';

const spec = JSON.parse(
  fs.readFileSync(
    new URL('../../../../spec/stress/capture-bounds.json', import.meta.url),
    'utf8',
  ),
);

const step = (s: RecordingState, e: LifecycleEvent) =>
  lifecycleReducer(s, e) ?? s;

function run() {
  const t0: number = spec.startedAt;
  let s = step(initialRecordingState(), {
    type: 'start-requested',
    meetingId: 'm1',
    at: t0,
    audioPath: '/r/m1.wav',
  });
  s = step(s, { type: 'start-acked' });
  for (let i = 0; i < spec.flaps; i++) {
    const since = i * spec.periodMs;
    s = step(s, { type: 'input-silent', source: spec.source, sinceMs: since });
    s = step(s, {
      type: 'input-audible',
      source: spec.source,
      at: t0 + since + spec.silentForMs,
    });
  }
  const reason = String(spec.reasonUnit).repeat(spec.reasonRepeat);
  for (let i = 0; i < spec.fallbacks; i++)
    s = step(s, {
      type: 'capture-fallback',
      from: reason,
      to: i % 2 ? reason : 'screencapturekit',
      reason,
      at: t0 + i,
    });
  return s;
}

describe('stress: capture record bounds (spec/stress/capture-bounds.json)', () => {
  it('limits match the spec', () => {
    expect(CAPTURE_LIMITS.silentSpans).toBe(spec.expect.silentSpans);
    expect(CAPTURE_LIMITS.fallbacks).toBe(spec.expect.fallbacks);
    expect(CAPTURE_LIMITS.reasonChars).toBe(spec.expect.reasonCodePoints);
  });

  it('hours of flapping keep the oldest 256 stretches; the row stays small', () => {
    const s = run();
    expect(s.silentSpans).toHaveLength(spec.expect.silentSpans);
    expect(s.silentSpans![0]).toEqual(spec.expect.first);
    expect(s.silentSpans!.at(-1)).toEqual(spec.expect.last);
    expect(s.silent).toBeUndefined();
    expect(JSON.stringify(s).length).toBeLessThan(
      spec.expect.maxSessionJsonBytes,
    );
  });

  it('a fallback loop keeps 8, each string cut to 200 code points with no lone surrogate', () => {
    const s = run();
    expect(s.fallbacks).toHaveLength(spec.expect.fallbacks);
    for (const f of s.fallbacks!)
      for (const v of [f.from, f.reason, f.to ?? '']) {
        expect(Array.from(v).length).toBeLessThanOrEqual(
          spec.expect.reasonCodePoints,
        );
        expect(v.isWellFormed()).toBe(true);
      }
  });

  it('a stretch open at a crash is still capped in the report', () => {
    let s = run();
    s = step(s, { type: 'input-silent', source: 'system', sinceMs: 1 });
    expect(s.silent).toHaveLength(1);
    const r = captureReport(s)!;
    expect(r.silentSpans).toHaveLength(spec.expect.reportSpansWithOpen);
    expect(r.fallbacks).toHaveLength(spec.expect.fallbacks);
  });
});
