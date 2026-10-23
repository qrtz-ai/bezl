import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import {
  WAVE_IDLE_AMP,
  WAVE_LIVE_FLOOR,
  Waveform,
  stepAmp,
  waveEnvelope,
  wavePath,
  waveTarget,
} from './Waveform';

const ys = (d: string) =>
  [...d.matchAll(/[ML][\d.]+ ([\d.-]+)/g)].map((m) => Number(m[1]));

describe('waveform math', () => {
  it('the target follows the mic level, with a floor while listening', () => {
    expect(waveTarget('live', 0)).toBeCloseTo(WAVE_LIVE_FLOOR);
    expect(waveTarget('live', 1)).toBeCloseTo(1);
    expect(waveTarget('live', 0.25)).toBeGreaterThan(waveTarget('live', 0.04));
    expect(waveTarget('live', 9)).toBe(1);
    expect(waveTarget('live', Number.NaN)).toBeCloseTo(WAVE_LIVE_FLOOR);
  });

  it('processing relaxes to the idle swell whatever the level', () => {
    expect(waveTarget('idle', 0)).toBe(WAVE_IDLE_AMP);
    expect(waveTarget('idle', 1)).toBe(WAVE_IDLE_AMP);
  });

  it('the drawn amplitude eases toward the target: fast attack, slower release, frame-rate independent', () => {
    const up = stepAmp(0, 1, 16);
    const down = 1 - stepAmp(1, 0, 16);
    expect(up).toBeGreaterThan(0);
    expect(up).toBeLessThan(1);
    expect(up).toBeGreaterThan(down);
    // two 8 ms frames land where one 16 ms frame does
    expect(stepAmp(stepAmp(0, 1, 8), 1, 8)).toBeCloseTo(stepAmp(0, 1, 16), 10);
    // it converges, and a stalled tab (huge dt) does not overshoot
    let a = 0;
    for (let i = 0; i < 120; i++) a = stepAmp(a, 0.5, 16);
    expect(a).toBeCloseTo(0.5, 3);
    expect(stepAmp(0, 0.5, 60_000)).toBeLessThanOrEqual(0.5);
  });

  it('the envelope pins both ends and peaks in the middle', () => {
    expect(waveEnvelope(0)).toBeCloseTo(0);
    expect(waveEnvelope(1)).toBeCloseTo(0);
    expect(waveEnvelope(0.5)).toBeCloseTo(1);
  });

  it('a path is flat at zero amplitude, stays inside the box, and moves with the phase', () => {
    const flat = ys(wavePath(160, 24, 0, 1, 2));
    expect(flat.every((y) => y === 12)).toBe(true);
    const loud = ys(wavePath(160, 24, 1, 0.3, 2));
    expect(Math.max(...loud)).toBeLessThanOrEqual(24);
    expect(Math.min(...loud)).toBeGreaterThanOrEqual(0);
    expect(Math.max(...loud) - Math.min(...loud)).toBeGreaterThan(12);
    expect(wavePath(160, 24, 0.5, 0, 2)).not.toBe(wavePath(160, 24, 0.5, 1, 2));
    expect(loud[0]).toBeCloseTo(12);
    expect(loud.at(-1)).toBeCloseTo(12);
  });

  it('renders three layered paths', () => {
    const html = renderToStaticMarkup(<Waveform level={0.5} />);
    expect(html).toContain('data-testid="talk-wave"');
    expect(html.match(/<path /g)).toHaveLength(3);
  });
});
