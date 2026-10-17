/**
 * Hardening cycle 6 (N32): the live view shows silent capture (N29). Before, `silent` / `noAudio` reached
 * note:session but no chip rendered them, so the user still found out after the meeting.
 */
import { describe, expect, it } from 'vitest';
import { SessionView } from '@qrtz/core';
import { captureChips } from './useRecording';

const view = (o: Partial<SessionView>) =>
  SessionView.parse({ phase: 'recording', ...o });
const ids = (o: Partial<SessionView>) => captureChips(view(o)).map((c) => c.id);

describe('captureChips (N32)', () => {
  it('nothing silent, nothing degraded: no chips', () => {
    expect(ids({})).toEqual([]);
  });
  it('one silent source: a chip naming it, after the degradations', () => {
    expect(
      ids({
        degraded: ['ax-unavailable'],
        silent: [{ source: 'mic', sinceMs: 61_000 }],
      }),
    ).toEqual(['ax-unavailable', 'silent-mic']);
    expect(
      captureChips(view({ silent: [{ source: 'system', sinceMs: 0 }] })),
    ).toEqual([{ id: 'silent-system', label: 'No sound from system audio' }]);
  });
  it('every delivered source silent: one "No sound" chip instead of one per source', () => {
    expect(
      ids({
        silent: [
          { source: 'system', sinceMs: 0 },
          { source: 'mic', sinceMs: 5 },
        ],
        noAudio: true,
      }),
    ).toEqual(['no-sound']);
    // mic-only + the mic silent: also no audio at all.
    expect(
      ids({
        degraded: ['mic-only'],
        silent: [{ source: 'mic', sinceMs: 0 }],
        noAudio: true,
      }),
    ).toEqual(['mic-only', 'no-sound']);
  });
  it('audio-unavailable already says it: no extra silence chip', () => {
    expect(
      ids({
        degraded: ['audio-unavailable'],
        silent: [{ source: 'mic', sinceMs: 0 }],
        noAudio: true,
      }),
    ).toEqual(['audio-unavailable']);
  });
  it('every chip id is unique (React keys, test ids)', () => {
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
