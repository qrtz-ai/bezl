import { useEffect, useRef, useState } from 'react';
import { Icon, type IconName } from './Icon';
import { ThemeScope, useTheme } from './theme';
import { Waveform } from './Waveform';

/**
 * ADR 0221: the notch surface's web preview, in the native surface's style (native/qrtz-helper/Sources/QrtzMacShared/
 * NotchSurface.swift): a black notch-shaped tab flush with the top edge drops down and out of the notch, its content
 * below the notch, then retracts into it. The shape comes from `notchPath`, the same steps as the Swift
 * `NotchShape.path` (NotchSurface.test.ts pins them). It is a mock for the gallery and Playwright; the real surface
 * is the helper's native panel. No clickable actions in v1.
 */

/** The fixed built-in icon set (module content names one; never a bitmap). */
export const NOTCH_ICONS = [
  'check',
  'alert',
  'info',
  'mic',
  'clock',
  'download',
  'bolt',
  'calendar',
] as const;
export type NotchIcon = (typeof NOTCH_ICONS)[number];

export type NotchPriority = 'system' | 'ctrl' | 'module';

/** What the surface shows (the wire `content` of `notch.show` / `notch.update`). */
export type NotchContent =
  | { kind: 'level'; busy: boolean }
  | { kind: 'status'; text: string; tone: 'normal' | 'error' }
  | { kind: 'progress'; label: string; value?: number }
  | { kind: 'icon'; icon: NotchIcon; label: string };

/** Sizes in pt/CSS px, the same as Swift `NotchGeometry`. */
export const NOTCH_GEOMETRY = {
  fakeNotchWidth: 180,
  fakeNotchHeight: 24,
  sideGrow: 70,
  dropGrow: 32,
  widenMax: 90,
  restRadius: 8,
  expandedRadius: 14,
  expandedEar: 8,
  padding: 14,
} as const;

/** Swift `NotchShape.clamp`. */
export function notchClamp(w: number, h: number, r: number, e: number) {
  w = Math.max(0, w);
  h = Math.max(0, h);
  const ear = Math.max(0, Math.min(e, h / 2, w / 4));
  const radius = Math.max(0, Math.min(r, h - ear, (w - 2 * ear) / 2));
  return { radius, ear };
}

/**
 * The notch outline as an SVG path (y runs down from the top edge): flat top, concave ears where the sides meet
 * the top, rounded bottom corners. `width` includes the ears. Ten steps, as Swift `NotchShape.path`.
 */
export function notchPath(
  width: number,
  height: number,
  radius: number,
  ear: number,
): string {
  const w = Math.max(0, width);
  const h = Math.max(0, height);
  const { radius: r, ear: e } = notchClamp(w, h, radius, ear);
  const n = (x: number) => +x.toFixed(2);
  return [
    `M${n(0)} ${n(0)}`,
    `Q${n(e)} ${n(0)} ${n(e)} ${n(e)}`,
    `L${n(e)} ${n(h - r)}`,
    `Q${n(e)} ${n(h)} ${n(e + r)} ${n(h)}`,
    `L${n(w - e - r)} ${n(h)}`,
    `Q${n(w - e)} ${n(h)} ${n(w - e)} ${n(h - r)}`,
    `L${n(w - e)} ${n(e)}`,
    `Q${n(w - e)} ${n(0)} ${n(w)} ${n(0)}`,
    `L${n(0)} ${n(0)}`,
    'Z',
  ].join(' ');
}

const ICON: Record<NotchIcon, IconName> = {
  check: 'check',
  alert: 'alert',
  info: 'info',
  mic: 'mic',
  clock: 'clock',
  download: 'download',
  bolt: 'automation',
  calendar: 'calendar',
};

const reducedMotion = () =>
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;

/**
 * A step response (0 to 1, at rest) of Apple's spring given as perceptual duration (s) and bounce, the same model as
 * `CASpringAnimation(perceptualDuration:bounce:)` / SwiftUI `.spring(duration:bounce:)`: ω0 = 2π / duration, damping
 * ratio 1 - bounce. Overshoots past 1 when bounce > 0. `t` in seconds.
 */
export function springStep(
  t: number,
  duration: number,
  bounce: number,
): number {
  if (t <= 0) return 0;
  const w0 = (2 * Math.PI) / duration;
  const z = 1 - bounce;
  if (z >= 1) return 1 - Math.exp(-w0 * t) * (1 + w0 * t);
  const wd = w0 * Math.sqrt(1 - z * z);
  return (
    1 -
    Math.exp(-z * w0 * t) *
      (Math.cos(wd * t) + ((z * w0) / wd) * Math.sin(wd * t))
  );
}

/**
 * The native surface's motion (NotchSurface.swift, ADR 0221). Show: the shape on a bouncy system spring
 * (perceptual 0.35 s, bounce 0.25), content fading in from 0.13 s over 0.10 s. Hide: content fades out over 0.06 s,
 * the shape retracts from 0.04 s on a no-bounce spring (perceptual 0.3 s). Seconds.
 */
export const NOTCH_MOTION = {
  expand: {
    duration: 0.35,
    bounce: 0.25,
    contentDelay: 0.13,
    contentFade: 0.1,
  },
  retract: { duration: 0.3, bounce: 0, delay: 0.04, contentFade: 0.06 },
} as const;

/** Close enough to rest to stop the frame loop. */
const settled = (t: number, d: number, b: number) =>
  t > d &&
  Math.abs(1 - springStep(t, d, b)) < 0.001 &&
  Math.abs(1 - springStep(t - 0.016, d, b)) < 0.001;

export function NotchSurface({
  content,
  source,
  open = true,
  notch = true,
  notchWidth = 188,
  notchHeight = 32,
  level = 0.5,
  'data-testid': testId = 'notch-surface',
}: {
  content: NotchContent;
  /** Module content: its registered display name, shown first (attribution). */
  source?: string;
  /** Expanded (true) or retracted into the notch. */
  open?: boolean;
  /** false: a screen without a notch, so a fake notch tab that rests at zero height. */
  notch?: boolean;
  notchWidth?: number;
  notchHeight?: number;
  level?: number;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const g = NOTCH_GEOMETRY;
  const nw = notch ? notchWidth : g.fakeNotchWidth;
  const top = notch ? notchHeight : g.fakeNotchHeight;
  const restH = notch ? notchHeight : 0;
  const panelW = nw + 2 * (g.sideGrow + g.widenMax + g.expandedEar);
  const panelH = top + g.dropGrow + 4;
  const [p, setP] = useState(open ? 1 : 0);
  const [c, setC] = useState(open ? 1 : 0);
  const from = useRef(p);
  const fromC = useRef(c);
  useEffect(() => {
    const target = open ? 1 : 0;
    if (reducedMotion() || typeof requestAnimationFrame !== 'function') {
      from.current = fromC.current = target;
      setP(target);
      setC(target);
      return;
    }
    const start = performance.now();
    const p0 = from.current;
    const c0 = fromC.current;
    const m = NOTCH_MOTION;
    let raf = 0;
    const frame = (now: number) => {
      const t = (now - start) / 1000;
      let k: number, kc: number, done: boolean;
      if (open) {
        const { duration: d, bounce: b, contentDelay, contentFade } = m.expand;
        k = springStep(t, d, b);
        // Interrupting a retract: the content comes back at once, as Swift's 0.04 s fromCurrent delay.
        const delay = p0 > 0 ? 0.04 : contentDelay;
        kc = Math.min(1, Math.max(0, (t - delay) / contentFade));
        done = settled(t, d, b) && kc >= 1;
      } else {
        const { duration: d, bounce: b, delay, contentFade } = m.retract;
        k = springStep(t - delay, d, b);
        kc = Math.min(1, t / contentFade);
        done = t > delay && settled(t - delay, d, b) && kc >= 1;
      }
      const v = done ? target : p0 + (target - p0) * k;
      const vc = c0 + (target - c0) * (1 - Math.pow(1 - kc, open ? 2 : 1));
      from.current = v;
      fromC.current = vc;
      setP(v);
      setC(vc);
      if (!done) raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [open]);

  const still = reducedMotion();
  const q = still ? 1 : p;
  const ew = nw + 2 * g.sideGrow + 2 * g.expandedEar;
  const w = nw + (ew - nw) * q;
  const h = restH + (top + g.dropGrow - restH) * q;
  const r = g.restRadius + (g.expandedRadius - g.restRadius) * q;
  const e = g.expandedEar * Math.max(0, q);
  const contentOpacity = still ? (open ? 1 : 0) : c;
  const text = (s: string) => (
    <span
      style={{
        overflow: 'hidden',
        textOverflow: 'ellipsis',
        whiteSpace: 'nowrap',
        minWidth: 0,
      }}
    >
      {source ? (
        <span style={{ color: 'rgba(255,255,255,0.55)' }}>{source} </span>
      ) : null}
      {s}
    </span>
  );
  const body = (() => {
    switch (content.kind) {
      case 'level':
        return <Waveform level={level} mode={content.busy ? 'idle' : 'live'} />;
      case 'status':
        return (
          <>
            {content.tone === 'error' ? <Icon name="alert" size="sm" /> : null}
            {text(content.text)}
          </>
        );
      case 'icon':
        return (
          <>
            <Icon name={ICON[content.icon]} size="sm" />
            {text(content.label)}
          </>
        );
      case 'progress':
        return (
          <>
            {text(content.label)}
            <span
              role="progressbar"
              aria-label={content.label}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={
                content.value === undefined
                  ? undefined
                  : Math.round(content.value * 100)
              }
              style={{
                flex: '0 0 64px',
                height: 3,
                borderRadius: 1.5,
                background: 'rgba(255,255,255,0.22)',
                overflow: 'hidden',
              }}
            >
              <span
                style={{
                  display: 'block',
                  height: 3,
                  width: `${Math.round((content.value ?? 0.3) * 100)}%`,
                  background: 'rgb(250,250,250)',
                }}
              />
            </span>
          </>
        );
    }
  })();
  return (
    <div
      data-testid={testId}
      data-kind={content.kind}
      data-open={open}
      data-notch={notch}
      style={{
        position: 'relative',
        width: panelW,
        height: panelH,
        pointerEvents: 'none',
        opacity: still ? (open ? 1 : 0) : 1,
        transition: still ? 'opacity 120ms' : undefined,
      }}
    >
      <svg
        aria-hidden
        width={panelW}
        height={panelH}
        viewBox={`0 0 ${panelW} ${panelH}`}
        style={{ position: 'absolute', inset: 0, display: 'block' }}
      >
        <path
          data-testid={`${testId}-shape`}
          transform={`translate(${(panelW - w) / 2} 0)`}
          d={notchPath(w, h, r, e)}
          fill="#000"
        />
      </svg>
      <div
        role="status"
        style={{
          position: 'absolute',
          left: (panelW - ew) / 2 + g.expandedEar + g.padding,
          right: (panelW - ew) / 2 + g.expandedEar + g.padding,
          top,
          height: g.dropGrow,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 8,
          color: 'rgb(250,250,250)',
          fontFamily: t.font.family,
          fontSize: '13.158px',
          opacity: contentOpacity,
        }}
      >
        {/* Light content on the black shape in both themes (the native surface's rule). */}
        <ThemeScope
          mode="dark"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            minWidth: 0,
            background: 'transparent',
          }}
        >
          {body}
        </ThemeScope>
      </div>
    </div>
  );
}
