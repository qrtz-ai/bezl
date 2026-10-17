/**
 * qrtz design tokens, derived from qrtz.ai (see docs/design-system.md, reference in docs/design-tokens.md):
 * neutral palette (shadcn "neutral"), geometric sans + mono accents, thin 1px borders instead of shadows,
 * small radii, fast ease-out motion. Dark is the brand default; light is an opt-in palette with the same roles.
 */
const darkColor = {
  bg: '#0a0a0a', // hsl(0 0% 3.9%)  — background
  surface: '#111111', // raised panels (sidebar)
  surfaceHover: '#171717',
  card: '#0a0a0a',
  border: '#262626', // hsl(0 0% 14.9%)
  borderStrong: '#404040',
  text: '#fafafa', // hsl(0 0% 98%)  — foreground
  textMuted: '#a3a3a3', // hsl(0 0% 63.9%) — muted-foreground
  textSubtle: '#737373',
  primary: '#fafafa', // shadcn dark primary
  primaryText: '#171717',
  accent: '#262626',
  ring: '#d4d4d4',
  danger: '#ef4444',
  success: '#22c55e',
  warning: '#f59e0b',
  recording: '#ef4444',
  selection: '#404040',
  scrollbar: '#262626',
  overlay: 'rgba(0,0,0,0.6)',
  // Back-compat aliases
  accentText: '#171717',
};

/** Same roles as dark (shadcn neutral light); muted text darkened to keep WCAG AA on white. */
const lightColor: Record<keyof typeof darkColor, string> = {
  bg: '#ffffff',
  surface: '#fafafa',
  surfaceHover: '#f5f5f5',
  card: '#ffffff',
  border: '#e5e5e5',
  borderStrong: '#d4d4d4',
  text: '#0a0a0a',
  textMuted: '#525252',
  textSubtle: '#6b6b6b',
  primary: '#171717',
  primaryText: '#fafafa',
  accent: '#f5f5f5',
  ring: '#404040',
  danger: '#b91c1c',
  success: '#166534',
  warning: '#92400e',
  recording: '#b91c1c',
  selection: '#d4d4d4',
  scrollbar: '#d4d4d4',
  overlay: 'rgba(0,0,0,0.3)',
  accentText: '#fafafa',
};

export type ThemeMode = 'dark' | 'light';

export const tokens = {
  mode: 'dark' as ThemeMode,
  color: darkColor as Record<keyof typeof darkColor, string>,
  space: {
    0: '0px',
    1: '4px',
    2: '8px',
    3: '12px',
    4: '16px',
    5: '20px',
    6: '24px',
    8: '32px',
    12: '48px',
    16: '64px',
  },
  radius: { sm: '4px', md: '6px', lg: '8px', full: '9999px' },
  font: {
    family:
      "'Geist', 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif",
    mono: "'Geist Mono', ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
    size: {
      xs: '12px',
      sm: '13px',
      md: '14px',
      lg: '18px',
      xl: '24px',
      '2xl': '30px',
      '3xl': '48px',
    },
    weight: { regular: 400, medium: 500, semibold: 600, bold: 700 },
    lineHeight: { tight: 1.2, normal: 1.5 },
  },
  motion: {
    fast: '120ms',
    normal: '180ms',
    slow: '280ms',
    easeOut: 'cubic-bezier(0.16, 1, 0.3, 1)',
    easeInOut: 'cubic-bezier(0.65, 0, 0.35, 1)',
    /** Durations by intent; components use these instead of raw ms. */
    instant: '0ms',
    hover: '120ms',
    enter: '180ms',
    exit: '120ms',
    pulse: '1400ms',
  },
  /** One focus ring everywhere: keyboard focus shows a 2px ring in `color.ring`, offset 2px. */
  focus: { width: '2px', offset: '2px', style: 'solid' },
  z: { base: 0, sticky: 10, overlay: 40, dialog: 50, toast: 60, tooltip: 70 },
} as const;

export type Tokens = typeof tokens;

/** Tokens for a palette; everything but color (and mode) is shared. */
export function themeTokens(mode: ThemeMode): Tokens {
  return mode === 'light' ? { ...tokens, mode, color: lightColor } : tokens;
}

/** CSS transition shorthand from motion tokens, e.g. transition('background', 'hover'). */
export function transition(
  prop: string | string[],
  speed: 'hover' | 'enter' | 'exit' = 'hover',
  t: Tokens = tokens,
): string {
  return (Array.isArray(prop) ? prop : [prop])
    .map((p) => `${p} ${t.motion[speed]} ${t.motion.easeOut}`)
    .join(', ');
}

const kebab = (s: string) => s.replace(/[A-Z]/g, (m) => '-' + m.toLowerCase());

/** Flattens tokens into CSS custom properties: --q-color-bg, --q-space-4, ... */
export function tokensToCssVars(t: Tokens = tokens): Record<string, string> {
  const out: Record<string, string> = {};
  const walk = (obj: Record<string, unknown>, prefix: string) => {
    for (const [k, v] of Object.entries(obj)) {
      const name = `${prefix}-${kebab(k)}`;
      if (typeof v === 'object' && v !== null)
        walk(v as Record<string, unknown>, name);
      else out[name] = String(v);
    }
  };
  walk(t as unknown as Record<string, unknown>, '--q');
  return out;
}

/** Global stylesheet: vars, reset, focus ring, scrollbars and the qrtz blink/underline motions. */
export function globalCss(t: Tokens = tokens): string {
  const vars = Object.entries(tokensToCssVars(t))
    .map(([k, v]) => `${k}:${v};`)
    .join('');
  return `:root{${vars}color-scheme:${t.mode};}
*,*::before,*::after{box-sizing:border-box;}
html,body,#root{height:100%;margin:0;}
body{background:var(--q-color-bg);color:var(--q-color-text);font-family:var(--q-font-family);font-size:var(--q-font-size-md);line-height:var(--q-font-line-height-normal);-webkit-font-smoothing:antialiased;}
:focus-visible{outline:var(--q-focus-width) var(--q-focus-style) var(--q-color-ring)!important;outline-offset:var(--q-focus-offset)!important;}
[data-q-no-ring]:focus-visible{outline:none!important;}
::selection{background:var(--q-color-selection);}
::-webkit-scrollbar{width:10px;height:10px}::-webkit-scrollbar-thumb{background:var(--q-color-scrollbar);border-radius:9999px;border:2px solid transparent;background-clip:content-box}
@keyframes q-blink{0%,49%{opacity:1}50%,100%{opacity:0}}
@keyframes q-spin{to{transform:rotate(360deg)}}
@keyframes q-fade-in{from{opacity:0;transform:translateY(4px)}to{opacity:1;transform:none}}
@keyframes q-pulse{0%,100%{opacity:1}50%{opacity:.4}}
.q-underline{position:relative}.q-underline::after{content:'';position:absolute;left:0;bottom:-2px;height:2px;width:100%;background:var(--q-color-primary);transform:scaleX(0);transform-origin:left;transition:transform var(--q-motion-normal) var(--q-motion-ease-out)}.q-underline:hover::after{transform:scaleX(1)}
@media (prefers-reduced-motion: reduce){*{animation-duration:var(--q-motion-instant)!important;transition-duration:var(--q-motion-instant)!important}}`;
}
