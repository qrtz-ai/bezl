/**
 * Hardening cycle 6 (N32): the live view shows silent capture (N29). Before, `silent` / `noAudio` reached
 * note:session but nothing rendered them, so the user still found out after the meeting. The chips became the
 * `CaptureStatus` silent / no-audio badges; `liveCaptureStatus` keeps the N32 rules.
 */
import { describe, expect, it } from 'vitest';
import { SessionView } from '@qrtz/core';
import { liveCaptureStatus } from './capture';

const view = (o: Partial<SessionView>) =>
  SessionView.parse({ phase: 'recording', startedAt: 0, ...o });
const ids = (o: Partial<SessionView>) => {
  const s = liveCaptureStatus(view(o));
  return [
    ...s.degraded.map((d) => d.id),
    ...s.silent.map((x) => `silent-${x.source}`),
    ...(s.noAudio ? ['no-audio'] : []),
  ];
};

describe('liveCaptureStatus (N32)', () => {
  it('nothing silent, nothing degraded: nothing', () => {
    expect(ids({})).toEqual([]);
  });
  it('one silent source: a badge naming it, after the degradations', () => {
    expect(
      ids({
        degraded: ['ax-unavailable'],
        silent: [{ source: 'mic', sinceMs: 61_000 }],
      }),
    ).toEqual(['ax-unavailable', 'silent-mic']);
    expect(
      liveCaptureStatus(view({ silent: [{ source: 'system', sinceMs: 0 }] }))
        .silent[0]!.source,
    ).toBe('system');
  });
  it('every delivered source silent: one no-audio badge instead of one per source', () => {
    expect(
      ids({
        silent: [
          { source: 'system', sinceMs: 0 },
          { source: 'mic', sinceMs: 5 },
        ],
        noAudio: true,
      }),
    ).toEqual(['no-audio']);
    // mic-only + the mic silent: also no audio at all.
    expect(
      ids({
        degraded: ['mic-only'],
        silent: [{ source: 'mic', sinceMs: 0 }],
        noAudio: true,
      }),
    ).toEqual(['mic-only', 'no-audio']);
  });
  it('audio-unavailable already says it: no extra silence badge', () => {
    expect(
      ids({
        degraded: ['audio-unavailable'],
        silent: [{ source: 'mic', sinceMs: 0 }],
        noAudio: true,
      }),
    ).toEqual(['audio-unavailable']);
  });
  it('every id is unique (React keys, test ids)', () => {
    const all = ids({
      degraded: ['ax-unavailable', 'transcript-gap', 'asr-unavailable'],
      silent: [
        { source: 'system', sinceMs: 0 },
        { source: 'mic', sinceMs: 0 },
      ],
    });
    expect(new Set(all).size).toBe(all.length);
  });
});
