import { describe, expect, it } from 'vitest';
import {
  activeSpeakerIntervals,
  mapSlotsToNames,
  nameSegmentsFromAx,
  normalizeSpeakerName,
} from './fusion';
import type { Segment } from './types';

const seg = (startMs: number, endMs: number, extra: Partial<Segment> = {}) =>
  ({ id: `${startMs}`, startMs, endMs, text: 'x', ...extra }) as Segment;

describe('speaker fusion', () => {
  it('normalizes names and treats placeholders as unknown', () => {
    expect(normalizeSpeakerName('  Ada\u200b  Lovelace ')).toBe('Ada Lovelace');
    for (const p of [
      '',
      ' ',
      'Unknown',
      'participant',
      'Guest',
      '...',
      '…',
      '?',
    ])
      expect(normalizeSpeakerName(p), p).toBeUndefined();
  });

  it('unknown events end the previous interval without creating one; repeats merge', () => {
    const iv = activeSpeakerIntervals(
      [
        { atMs: 0, name: 'Ada' },
        { atMs: 1000, name: 'Ada ' },
        { atMs: 2000, name: '' },
        { atMs: 3000, name: 'Alan' },
      ],
      4000,
    );
    expect(iv).toEqual([
      { name: 'Ada', startMs: 0, endMs: 2000 },
      { name: 'Alan', startMs: 3000, endMs: 4000 },
    ]);
  });

  it('compensates AX highlight lag when mapping slots', () => {
    // AX switches 600 ms after each real turn change.
    const turns = [
      { slot: 'S0', startMs: 0, endMs: 2000 },
      { slot: 'S1', startMs: 2000, endMs: 3000 },
      { slot: 'S0', startMs: 3000, endMs: 5000 },
      { slot: 'S1', startMs: 5000, endMs: 6000 },
    ];
    const ax = [
      { atMs: 600, name: 'Ada' },
      { atMs: 2600, name: 'Alan' },
      { atMs: 3600, name: 'Ada' },
      { atMs: 5600, name: 'Alan' },
    ];
    const m = mapSlotsToNames(turns, ax, { endMs: 6000, lagMs: 600 });
    expect(Object.fromEntries(m)).toEqual({ S0: 'Ada', S1: 'Alan' });
  });

  it('overlapping speech: an interrupter who only talks over someone does not steal their name', () => {
    const turns = [
      { slot: 'S0', startMs: 0, endMs: 10_000 }, // Ada talks throughout
      { slot: 'S1', startMs: 2000, endMs: 5000 }, // Alan interjects over her
      { slot: 'S1', startMs: 10_000, endMs: 12_000 }, // then talks alone
    ];
    const ax = [
      { atMs: 0, name: 'Ada' },
      { atMs: 10_000, name: 'Alan' },
    ];
    const m = mapSlotsToNames(turns, ax, { endMs: 12_000, lagMs: 0 });
    expect(m.get('S0')).toBe('Ada');
    expect(m.get('S1')).toBe('Alan');
  });

  it('unknown speakers stay unnamed instead of borrowing a neighbour', () => {
    const turns = [
      { slot: 'S0', startMs: 0, endMs: 4000 },
      { slot: 'S2', startMs: 4000, endMs: 8000 }, // dial-in with no AX tile
    ];
    const ax = [
      { atMs: 0, name: 'Ada' },
      { atMs: 4000, name: 'Unknown' },
    ];
    const m = mapSlotsToNames(turns, ax, { endMs: 8000, lagMs: 0 });
    expect(m.get('S0')).toBe('Ada');
    expect(m.has('S2')).toBe(false);
  });

  it('two slots never get the same name, and weak evidence is ignored', () => {
    const turns = [
      { slot: 'S0', startMs: 0, endMs: 5000 },
      { slot: 'S1', startMs: 5000, endMs: 10_000 },
    ];
    const ax = [{ atMs: 0, name: 'Ada' }];
    const m = mapSlotsToNames(turns, ax, { endMs: 10_000, lagMs: 0 });
    expect([...m.values()]).toEqual(['Ada']);
    const weak = mapSlotsToNames(
      [{ slot: 'S0', startMs: 0, endMs: 10_000 }],
      [{ atMs: 9000, name: 'Ada' }],
      { endMs: 10_000, lagMs: 0 },
    );
    expect(weak.size).toBe(0);
  });

  it('live naming: lag-shifted, keeps the existing label when AX evidence is thin or unknown', () => {
    const ax = [
      { atMs: 300, name: 'Ada' },
      { atMs: 2300, name: '' },
      { atMs: 4300, name: 'Alan' },
    ];
    const out = nameSegmentsFromAx(
      [
        seg(0, 2000),
        seg(2000, 4000, { speakerName: 'Speaker 2' }),
        seg(4000, 6000),
        seg(6000, 6100, { speakerName: 'Grace' }),
      ],
      ax,
      6100,
      300,
    );
    expect(out.map((s) => s.speakerName)).toEqual([
      'Ada',
      'Speaker 2',
      'Alan',
      'Alan',
    ]);
    // A segment barely touched by a highlight keeps its own label.
    const thin = nameSegmentsFromAx(
      [seg(0, 4000, { speakerName: 'Grace' })],
      [{ atMs: 3900, name: 'Ada' }],
      4000,
      0,
    );
    expect(thin[0]!.speakerName).toBe('Grace');
  });
});
