import { describe, expect, it } from 'vitest';
import type { SessionView } from '@qrtz/core';
import {
  CAPTURE_NOTICE_MAX,
  captureNoticeLines,
  fmtTimeOfDay,
  silentBadges,
} from './capture';

// Local wall-clock times, whatever the TZ the suite runs in.
const at = (h: number, m: number) => new Date(2026, 9, 17, h, m).getTime();
const start = at(10, 0);

describe('capture status (N29, H8)', () => {
  it('formats a time of day as 24 h hh:mm', () => {
    expect(fmtTimeOfDay(at(10, 4))).toBe('10:04');
    expect(fmtTimeOfDay(at(15, 30))).toBe('15:30');
  });

  it('live badges: one per silent source, since = startedAt + sinceMs', () => {
    const s = {
      startedAt: start,
      silent: [
        { source: 'mic', sinceMs: 4 * 60_000 },
        { source: 'system', sinceMs: 12 * 60_000 },
      ],
    } as unknown as SessionView;
    expect(silentBadges(s)).toEqual([
      { source: 'mic', since: '10:04' },
      { source: 'system', since: '10:12' },
    ]);
    expect(silentBadges({ startedAt: start } as SessionView)).toEqual([]);
  });

  it('meeting notice: short lowercase lines for silences and fallbacks', () => {
    const lines = captureNoticeLines(start, {
      silentSpans: [
        { source: 'mic', startMs: 4 * 60_000, endMs: 9 * 60_000 },
        { source: 'system', startMs: 20 * 60_000 },
        { source: 'mic', startMs: 30 * 60_000, endMs: 30 * 60_000 }, // taken away: shown as a degradation
      ],
      fallbacks: [
        {
          from: 'core-audio-tap',
          to: 'screencapturekit',
          reason: 'OSStatus 1',
          atMs: 0,
        },
        {
          from: 'core-audio-tap',
          to: 'screencapturekit',
          reason: 'OSStatus 1',
          atMs: 60_000,
        },
        { from: 'screencapturekit', to: null, reason: 'denied', atMs: 61_000 },
        { from: 'Some-Source', to: null, reason: 'x', atMs: 62_000 },
      ],
    });
    expect(lines.map((l) => l.text)).toEqual([
      'no audio from mic 10:04–10:09',
      'no audio from system audio after 10:20',
      'meeting audio tap failed, used screen capture ×2',
      'screen capture failed, no audio source left',
      'some-source failed, no audio source left',
    ]);
    for (const l of lines) expect(l.text).toBe(l.text.toLowerCase());
    expect(lines[2]!.detail).toBe('OSStatus 1');
    expect(new Set(lines.map((l) => l.id)).size).toBe(lines.length);
  });

  it('meeting notice: nothing to say → no lines; many → capped with +n more', () => {
    expect(
      captureNoticeLines(start, {
        deviceFailures: [
          { degraded: ['no-mic'], reason: 'device busy', atMs: 7 * 60_000 },
        ],
      }),
    ).toEqual([
      {
        id: 'device-0',
        text: 'mic device failed at 10:07',
        detail: 'device busy',
      },
    ]); // N35: a device failure, never worded as a revoke
    expect(captureNoticeLines(start, undefined)).toEqual([]);
    expect(captureNoticeLines(start, {})).toEqual([]);
    const many = captureNoticeLines(start, {
      silentSpans: Array.from({ length: 10 }, (_, i) => ({
        source: 'mic' as const,
        startMs: i * 120_000,
        endMs: i * 120_000 + 60_000,
      })),
    });
    expect(many).toHaveLength(CAPTURE_NOTICE_MAX);
    expect(many.at(-1)).toEqual({ id: 'more', text: '+5 more' });
  });

  it('meeting notice: a lost stretch says the source was not recorded, unless a device or fallback line already says it (N39)', () => {
    const min = 60_000;
    const lines = captureNoticeLines(start, {
      deviceFailures: [
        { degraded: ['no-mic'], reason: 'device busy', atMs: 7 * min },
      ],
      fallbacks: [
        { from: 'screencapturekit', to: null, reason: 'busy', atMs: 40 * min },
      ],
      lostSpans: [
        { source: 'mic', startMs: 7 * min, endMs: 9 * min }, // the device line says it
        { source: 'mic', startMs: 12 * min, endMs: 20 * min }, // a revoke
        { source: 'system', startMs: 25 * min, endMs: 30 * min }, // screen recording revoked
        { source: 'mic', startMs: 32 * min, endMs: 35 * min }, // both together: one line
        { source: 'system', startMs: 32 * min, endMs: 35 * min },
        { source: 'mic', startMs: 40 * min }, // the "no source left" line says it
        { source: 'system', startMs: 40 * min },
        { source: 'system', startMs: 50 * min }, // still lost when the app died
        { source: 'mic', startMs: 55 * min, endMs: 55 * min }, // zero length: nothing to say
      ],
    });
    expect(lines.map((l) => l.text)).toEqual([
      'screen capture failed, no audio source left',
      'mic device failed at 10:07',
      'mic not recorded 10:12–10:20',
      'system audio not recorded 10:25–10:30',
      'nothing recorded 10:32–10:35',
      'system audio not recorded after 10:50',
    ]);
    const all = captureNoticeLines(start, {
      lostSpans: [
        { source: 'mic', startMs: 32 * min, endMs: 35 * min },
        { source: 'system', startMs: 32 * min, endMs: 35 * min },
        { source: 'system', startMs: 50 * min },
      ],
    });
    expect(all).toEqual([
      { id: 'lost-0', text: 'nothing recorded 10:32–10:35' },
      { id: 'lost-2', text: 'system audio not recorded after 10:50' },
    ]);
    for (const l of [...lines, ...all])
      expect(l.text).toBe(l.text.toLowerCase());
  });
});
