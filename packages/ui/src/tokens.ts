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
  dangerSolid: '#dc2626', // danger button fill, 4.8:1 with dangerSolidText (WCAG AA)
  dangerSolidText: '#ffffff',
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
  dangerSolid: '#b91c1c',
  dangerSolidText: '#ffffff',
  success: '#166534',
  warning: '#92400e',
  recording: '#b91c1c',
  selection: '#d4d4d4',
  scrollbar: '#d4d4d4',
  overlay: 'rgba(0,0,0,0.3)',
  accentText: '#fafafa',
};

/** The 4px spacing scale; every gap, padding and margin in @qrtz/ui is one of these. */
const space = {
  0: '0px',
  0.5: '2px', // hairline gaps inside a row (title over its caption)
  1: '4px',
  2: '8px',
  3: '12px',
  4: '16px',
  5: '20px',
  6: '24px',
  8: '32px',
  12: '48px',
  16: '64px',
} as const;

export type ThemeMode = 'dark' | 'light';

export const tokens = {
  mode: 'dark' as ThemeMode,
  color: darkColor as Record<keyof typeof darkColor, string>,
  space,
  radius: { xs: '2px', sm: '4px', md: '6px', lg: '8px', full: '9999px' },
  /**
   * One family everywhere: Geist Pixel Square, bundled locally from Vercel's `geist` 1.8.1 release (OFL-1.1).
   * It ships one weight and draws on a 38/1000 em pixel grid, so sizes are multiples of 13.158px (one font
   * pixel = 0.5 CSS px = one device pixel on Retina). Hierarchy comes from size and color, never fake bold.
   */
  font: {
    family:
      "'Geist Pixel Square', ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
    /** The only weight the font has; every type style uses it (font-synthesis is off). */
    weight: 500,
  },
  /** The type scale. Use <Text variant>, <Heading>, <Label>, <Code>, <Mono> or typeStyle(); never raw values. */
  type: {
    display: { size: '52.632px', lineHeight: '64px', tracking: '0em' },
    h1: { size: '39.474px', lineHeight: '48px', tracking: '0em' },
    h2: { size: '26.316px', lineHeight: '32px', tracking: '0em' },
    h3: { size: '19.737px', lineHeight: '24px', tracking: '0em' },
    bodyLg: { size: '19.737px', lineHeight: '28px', tracking: '0em' },
    body: { size: '13.158px', lineHeight: '20px', tracking: '0.019em' },
    bodySm: { size: '13.158px', lineHeight: '18px', tracking: '0em' },
    caption: { size: '13.158px', lineHeight: '16px', tracking: '0em' },
    label: { size: '13.158px', lineHeight: '16px', tracking: '0.038em' },
    mono: { size: '13.158px', lineHeight: '20px', tracking: '0em' },
    code: { size: '13.158px', lineHeight: '20px', tracking: '0em' },
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
    blink: '1100ms',
    spin: '700ms',
  },
  /** Feather icon sizes (px squares) and stroke; use <Icon size>, never raw numbers. */
  icon: {
    size: { xs: 12, sm: 14, md: 16, lg: 20, xl: 32 },
    stroke: 2,
  },
  /** One focus ring everywhere: keyboard focus shows a 2px ring in `color.ring`, offset 2px. */
  focus: { width: '2px', offset: '2px', style: 'solid' },
  /** The one shadow, for things that float over content (dialog, menu, toast). Everything else uses 1px borders. */
  shadow: { overlay: '0 12px 32px rgba(0,0,0,0.35)' },
  /**
   * The notch is always a black hardware surface, in every theme and contrast mode,
   * so it has its own fixed on-black palette instead of following `color`.
   */
  notch: {
    bg: '#000000',
    text: 'rgb(250,250,250)',
    textMuted: 'rgba(255,255,255,0.55)',
    track: 'rgba(255,255,255,0.22)',
  },
  /** Shared layout geometry (px numbers), used by Page, SideNav, SplitView and CardGrid. */
  layout: {
    pageMax: 880,
    sidenav: 176,
    cardMin: 220,
    pagePadTop: 40,
    pageGutter: 32,
    pagePadBottom: 48,
    pageHeader: 40,
    splitSidebar: 240,
    notesSidebar: 200,
    notesMinHeight: 420,
    notesCompactMinHeight: 220,
    notesCompactHeight: 260,
    /** The notes frame fills the window less the page chrome above and below it. */
    notesChrome: 260,
    paneLoadingMin: 120,
    paletteMaxHeight: 360,
    paletteInput: 48,
    paletteHeader: 56,
    compactListMax: 140,
    notesCompactListMax: 110,
  },
  /** Borders: one 1px hairline everywhere, a 2px thick edge for the active tab, spinner and key caps. */
  border: { hairline: '1px', thick: '2px' },
  /** Component sizes (px numbers) that are not on the spacing scale. */
  size: {
    dotSm: 6,
    dot: 8,
    badgeInset: 3,
    iconButton: 26,
    appIcon: { sm: 32, md: 40, lg: 56 },
    talkInlineMin: 44,
    waveStroke: { main: 2, layer: 1.5 },
    control: { sm: 28, md: 34, lg: 40 },
    row: { sm: 28, md: 32, lg: 52 },
    audio: 32,
    meter: { width: 48, height: 3 },
    bar: 4,
    progress: 6,
    switch: { width: 32, height: 18, thumb: 14, inset: 2, on: 16 },
    srOnly: 1,
    menuMin: 180,
    moreMenuMin: 200,
    moreMenuOffset: 30,
    toastMin: 240,
    tooltipOffset: 6,
    textareaMin: 80,
    calloutBody: 160,
    rowProgress: 72,
    sliderValue: 52,
    lifecycleLabel: 120,
    ctrlScore: 80,
    ctrlPct: 36,
    timeColumn: 56,
    listRowIntrinsic: 44,
    underlineOffset: '3px',
  },
  /** The design-system gallery's specimen sizes (px numbers). */
  gallery: {
    typeName: 96,
    typeMeta: 180,
    swatch: 96,
    swatchHeight: 40,
    narrow: 140,
    control: 160,
    panel: 320,
    tile: 40,
    tileMin: 120,
    tick: 16,
  },
  opacity: { disabled: 0.5, muted: 0.6 },
  blur: { overlay: 'blur(2px)', blocked: 'blur(6px)' },
  /**
   * The opts grid on every page: label left, control in a fixed right column, one row height, and the gaps
   * between a section's heading and its rows and between rows (sections themselves are PAGE.gap apart).
   */
  settings: {
    controlWidth: 240,
    rowMinHeight: 36,
    titleGap: space[1],
    headerGap: space[3],
    rowGap: space[2],
    labelGap: space[4],
  },
  z: { base: 0, sticky: 10, overlay: 40, dialog: 50, toast: 60, tooltip: 70 },
} as const;

export type Tokens = typeof tokens;
export type TypeVariant = keyof Tokens['type'];
export const TYPE_VARIANTS = Object.keys(tokens.type) as TypeVariant[];

/** Inline style for a type variant: family, size, weight, line-height, tracking (all from tokens). */
export function typeStyle(
  v: TypeVariant,
  t: Tokens = tokens,
): {
  fontFamily: string;
  fontSize: string;
  fontWeight: number;
  lineHeight: string;
  letterSpacing: string;
} {
  const s = t.type[v];
  return {
    fontFamily: t.font.family,
    fontSize: s.size,
    fontWeight: t.font.weight,
    lineHeight: s.lineHeight,
    letterSpacing: s.tracking,
  };
}

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
      const name = `${prefix}-${kebab(k).replace('.', '_')}`; // space 0.5 → --q-space-0_5
      if (typeof v === 'object' && v !== null)
        walk(v as Record<string, unknown>, name);
      else out[name] = String(v);
    }
  };
  walk(t as unknown as Record<string, unknown>, '--q');
  return out;
}

/**
 * Global stylesheet: vars, reset, focus ring, scrollbars and the qrtz blink/underline motions.
 *
 * Every page scroll root (`[data-scroll-root]`: the AppFrame main, the onboarding frame) always shows its vertical
 * scrollbar track (`overflow-y: scroll`), so every page gets the same content width whether it overflows or not.
 * `scrollbar-gutter: stable` alone is not enough: WebKit (the macOS WKWebView qrtz ships in) ignores it for the
 * styled `::-webkit-scrollbar` below, which always takes 10px of layout on macOS, so a page that scrolled (the qrtz
 * page) lost 10px and its centered 880px column moved 5px against one that didn't. The thumb only draws when the
 * page actually scrolls; the track is transparent.
 */
export function globalCss(t: Tokens = tokens, fontUrl?: string): string {
  const vars = Object.entries(tokensToCssVars(t))
    .map(([k, v]) => `${k}:${v};`)
    .join('');
  const face = fontUrl
    ? `@font-face{font-family:'Geist Pixel Square';src:url(${fontUrl}) format('woff2');font-weight:100 900;font-style:normal;font-display:block;}`
    : '';
  return `${face}:root{${vars}color-scheme:${t.mode};}
*,*::before,*::after{box-sizing:border-box;}
html,body,#root{height:100%;margin:0;overflow-x:hidden;overscroll-behavior:none;}
body{overflow-wrap:break-word;background:var(--q-color-bg);color:var(--q-color-text);font-family:var(--q-font-family);font-weight:var(--q-font-weight);font-size:var(--q-type-body-size);line-height:var(--q-type-body-line-height);letter-spacing:var(--q-type-body-tracking);font-synthesis:none;-webkit-font-smoothing:antialiased;}
button,input,select,textarea,option,code,kbd,pre,samp{font:inherit;letter-spacing:inherit;color:inherit;}
h1,h2,h3,h4,h5,h6,strong,b,th{font-weight:inherit;}
:focus-visible{outline:var(--q-focus-width) var(--q-focus-style) var(--q-color-ring)!important;outline-offset:var(--q-focus-offset)!important;}
[data-q-no-ring]:focus-visible{outline:none!important;}
::selection{background:var(--q-color-selection);}
[data-scroll-root]{overflow-y:scroll!important;scrollbar-gutter:stable;}
::-webkit-scrollbar{width:10px;height:10px}::-webkit-scrollbar-thumb{background:var(--q-color-scrollbar);border-radius:9999px;border:2px solid transparent;background-clip:content-box}
@keyframes q-blink{0%,49%{opacity:1}50%,100%{opacity:0}}
@keyframes q-spin{to{transform:rotate(360deg)}}
@keyframes q-fade-in{from{opacity:0;transform:translateY(4px)}to{opacity:1;transform:none}}
@keyframes q-pulse{0%,100%{opacity:1}50%{opacity:.4}}
html,body,#root{height:100%;margin:0;}
.q-drag{position:fixed;inset:0 0 auto 0;height:32px;z-index:1;-webkit-app-region:drag;}
.q-drag-all{-webkit-app-region:drag;}.q-no-drag{-webkit-app-region:no-drag;}
.q-underline{position:relative}.q-underline::after{content:'';position:absolute;left:0;bottom:-2px;height:2px;width:100%;background:var(--q-color-primary);transform:scaleX(0);transform-origin:left;transition:transform var(--q-motion-normal) var(--q-motion-ease-out)}.q-underline:hover::after{transform:scaleX(1)}
@media (prefers-reduced-motion: reduce){*{animation-duration:var(--q-motion-instant)!important;transition-duration:var(--q-motion-instant)!important}}`;
}
