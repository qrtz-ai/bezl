import { describe, expect, it } from 'vitest';
import { TranscriptAssembler } from './transcript';
import { labelSegments, mapSlotsToNames } from './fusion';

describe('TranscriptAssembler', () => {
  it('replaces partials, finalizes, ignores late partials and sorts', () => {
    const a = new TranscriptAssembler();
    a.push({ kind: 'partial', utteranceId: 'u2', startMs: 2000, text: 'wor' });
    a.push({ kind: 'partial', utteranceId: 'u1', startMs: 0, text: 'hel' });
    a.push({ kind: 'partial', utteranceId: 'u1', startMs: 0, text: 'hello' });
    expect(a.livePartials().map((p) => p.text)).toEqual(['hello', 'wor']);
    a.push({
      kind: 'final',
      utteranceId: 'u2',
      startMs: 2000,
      endMs: 2500,
      text: ' world ',
    });
    a.push({
      kind: 'final',
      utteranceId: 'u1',
      startMs: 0,
      endMs: 900,
      text: 'Hello.',
    });
    a.push({ kind: 'partial', utteranceId: 'u1', startMs: 0, text: 'stale' });
    expect(a.segments().map((s) => s.text)).toEqual(['Hello.', 'world']);
    expect(a.livePartials()).toEqual([]);
    expect(a.tail(8)).toBe('…. world');
  });
  it('drops empty finals', () => {
    const a = new TranscriptAssembler();
    a.push({
      kind: 'final',
      utteranceId: 'x',
      startMs: 0,
      endMs: 1,
      text: '  ',
    });
    expect(a.segments()).toEqual([]);
  });
});

describe('diarization fusion', () => {
  const turns = [
    { slot: 'S0', startMs: 0, endMs: 4000 },
    { slot: 'S1', startMs: 4000, endMs: 9000 },
    { slot: 'S0', startMs: 9000, endMs: 12000 },
    { slot: 'S2', startMs: 12000, endMs: 12300 },
  ];
  // AX highlight lags ~300ms and briefly flickers.
  const active = [
    { atMs: 300, name: 'Ada' },
    { atMs: 4300, name: 'Grace' },
    { atMs: 6000, name: 'Ada' },
    { atMs: 6200, name: 'Grace' },
    { atMs: 9300, name: 'Ada' },
  ];
  it('maps slots one-to-one by overlap and leaves weak slots unnamed', () => {
    const m = mapSlotsToNames(turns, active, { endMs: 12300 });
    expect(Object.fromEntries(m)).toEqual({ S0: 'Ada', S1: 'Grace' });
  });
  it('labels segments with slot + name', () => {
    const names = mapSlotsToNames(turns, active, { endMs: 12300 });
    const segs = labelSegments(
      [
        { id: 'a', startMs: 100, endMs: 3000, text: 'hi', source: 'post' },
        { id: 'b', startMs: 5000, endMs: 8000, text: 'yo', source: 'post' },
        { id: 'c', startMs: 12050, endMs: 12250, text: 'hm', source: 'post' },
      ],
      turns,
      names,
    );
    expect(segs.map((s) => [s.speaker, s.speakerName])).toEqual([
      ['S0', 'Ada'],
      ['S1', 'Grace'],
      ['S2', undefined],
    ]);
  });
});
