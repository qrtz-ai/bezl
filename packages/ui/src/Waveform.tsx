import { useEffect, useRef } from 'react';
import { useTheme } from './theme';
import { tokens } from './tokens';

/**
 * Live mic waveform for the echo pill: a few layered sine waves under a tapered envelope whose amplitude follows the
 * mic level. The level arrives at about 20 Hz (helper `dictation.level`); every animation frame eases the drawn
 * amplitude toward it (fast attack, slower release) and advances the phase, so the wave flows between updates.
 * `idle` (processing) relaxes to a slow, low swell. The frame loop writes the path `d` attributes directly, so
 * React does not re-render 60 times a second. With reduced motion the phase stands still; the amplitude still
 * follows the level.
 */

export type WaveMode = 'live' | 'idle';

/** Smallest amplitude while listening, so silence still reads as "listening" rather than a dead line. */
export const WAVE_LIVE_FLOOR = 0.06;
/** Amplitude of the processing swell. */
export const WAVE_IDLE_AMP = 0.14;
/** Attack / release time constants (ms) for the drawn amplitude. */
export const WAVE_ATTACK_MS = 60;
export const WAVE_RELEASE_MS = 220;

/** Phase speed in radians per second for each mode. */
const SPEED: Record<WaveMode, number> = { live: 7, idle: 2.2 };
/** The layers: cycles across the width, amplitude share, opacity, phase speed multiplier, phase offset. */
export const WAVE_LAYERS = [
  { cycles: 1.6, gain: 1, opacity: 1, rate: 1, offset: 0 }, // tokens-allow: canvas layer data, not a style
  { cycles: 2.3, gain: 0.66, opacity: 0.5, rate: -1.3, offset: 1.9 }, // tokens-allow: canvas layer data, not a style
  { cycles: 3.1, gain: 0.4, opacity: 0.28, rate: 1.7, offset: 4.1 }, // tokens-allow: canvas layer data, not a style
] as const;

const clamp01 = (x: number) =>
  Number.isFinite(x) ? Math.min(1, Math.max(0, x)) : 0;

/** The amplitude (0..1 of half the height) the wave is easing toward. `level` is the 0..1 mic RMS. */
export function waveTarget(mode: WaveMode, level: number): number {
  if (mode === 'idle') return WAVE_IDLE_AMP;
  // sqrt: speech RMS sits low in 0..1; this lifts normal talking to a visible swing without clipping shouts.
  return WAVE_LIVE_FLOOR + (1 - WAVE_LIVE_FLOOR) * Math.sqrt(clamp01(level));
}

/** One frame of exponential smoothing from `cur` toward `target` over `dtMs`. Frame-rate independent. */
export function stepAmp(cur: number, target: number, dtMs: number): number {
  const dt = Math.max(0, Math.min(dtMs, 250));
  const tau = target > cur ? WAVE_ATTACK_MS : WAVE_RELEASE_MS;
  return target + (cur - target) * Math.exp(-dt / tau);
}

/** Envelope across the width (0 at both ends, 1 in the middle), so the wave is pinned at the edges. */
export const waveEnvelope = (u: number) =>
  Math.pow(Math.sin(Math.PI * clamp01(u)), 1.6);

/** SVG path of one sine layer: `amp` 0..1 of half the height, `phase` in radians, `cycles` across the width. */
export function wavePath(
  width: number,
  height: number,
  amp: number,
  phase: number,
  cycles: number,
  points = 48,
): string {
  const mid = height / 2;
  const a = clamp01(amp) * (mid - 1);
  let d = '';
  for (let i = 0; i <= points; i++) {
    const u = i / points;
    const y =
      mid + a * waveEnvelope(u) * Math.sin(2 * Math.PI * cycles * u + phase);
    d += `${i ? 'L' : 'M'}${(u * width).toFixed(1)} ${y.toFixed(2)}`;
  }
  return d;
}

function layerPaths(
  width: number,
  height: number,
  amp: number,
  phase: number,
): string[] {
  return WAVE_LAYERS.map((l) =>
    wavePath(width, height, amp * l.gain, phase * l.rate + l.offset, l.cycles),
  );
}

const reducedMotion = () =>
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;

export function Waveform({
  level,
  mode = 'live',
  width = 160,
  height = 24,
}: {
  /** Mic RMS, 0..1. */
  level: number;
  mode?: WaveMode;
  width?: number;
  height?: number;
}) {
  const t = useTheme();
  const target = useRef(waveTarget(mode, level));
  target.current = waveTarget(mode, level);
  const speed = useRef(SPEED[mode]);
  speed.current = SPEED[mode];
  const paths = useRef<(SVGPathElement | null)[]>([]);
  const amp = useRef(target.current);

  useEffect(() => {
    if (typeof requestAnimationFrame !== 'function') return;
    const still = reducedMotion();
    let phase = 0;
    let last: number | undefined;
    let raf = 0;
    const frame = (now: number) => {
      const dt = last === undefined ? 16 : now - last;
      last = now;
      amp.current = stepAmp(amp.current, target.current, dt);
      if (!still)
        phase = (phase + (speed.current * dt) / 1000) % (Math.PI * 200);
      const ds = layerPaths(width, height, amp.current, phase);
      ds.forEach((d, i) => paths.current[i]?.setAttribute('d', d));
      raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [width, height]);

  const initial = layerPaths(width, height, amp.current, 0);
  return (
    <svg
      aria-hidden
      data-testid="talk-wave"
      data-mode={mode}
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      style={{ display: 'block', overflow: 'visible' }}
    >
      {WAVE_LAYERS.map((l, i) => (
        <path
          key={i}
          ref={(el) => {
            paths.current[i] = el;
          }}
          d={initial[i]}
          fill="none"
          stroke={t.color.text}
          strokeOpacity={l.opacity}
          strokeWidth={
            i === 0 ? tokens.size.waveStroke.main : tokens.size.waveStroke.layer
          }
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      ))}
    </svg>
  );
}
