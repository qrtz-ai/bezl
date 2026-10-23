# @qrtz/ui design tokens

<!-- Generated from packages/ui/src/tokens.ts by tokens-doc.test.ts (UPDATE_TOKENS_DOC=1 bun run test). Do not edit by hand. -->

Use tokens through `useTheme()` in components, or the CSS custom properties that `ThemeProvider` injects.
Dark is the qrtz.ai brand default. Light is an opt-in palette with the same roles; choose it in opts → appearance (Dark / Light / System).

## Color

| Token | CSS variable | Dark | Light |
|---|---|---|---|
| `color.bg` | `--q-color-bg` | `#0a0a0a` | `#ffffff` |
| `color.surface` | `--q-color-surface` | `#111111` | `#fafafa` |
| `color.surfaceHover` | `--q-color-surface-hover` | `#171717` | `#f5f5f5` |
| `color.card` | `--q-color-card` | `#0a0a0a` | `#ffffff` |
| `color.border` | `--q-color-border` | `#262626` | `#e5e5e5` |
| `color.borderStrong` | `--q-color-border-strong` | `#404040` | `#d4d4d4` |
| `color.text` | `--q-color-text` | `#fafafa` | `#0a0a0a` |
| `color.textMuted` | `--q-color-text-muted` | `#a3a3a3` | `#525252` |
| `color.textSubtle` | `--q-color-text-subtle` | `#737373` | `#6b6b6b` |
| `color.primary` | `--q-color-primary` | `#fafafa` | `#171717` |
| `color.primaryText` | `--q-color-primary-text` | `#171717` | `#fafafa` |
| `color.accent` | `--q-color-accent` | `#262626` | `#f5f5f5` |
| `color.ring` | `--q-color-ring` | `#d4d4d4` | `#404040` |
| `color.danger` | `--q-color-danger` | `#ef4444` | `#b91c1c` |
| `color.dangerSolid` | `--q-color-danger-solid` | `#dc2626` | `#b91c1c` |
| `color.dangerSolidText` | `--q-color-danger-solid-text` | `#ffffff` | `#ffffff` |
| `color.success` | `--q-color-success` | `#22c55e` | `#166534` |
| `color.warning` | `--q-color-warning` | `#f59e0b` | `#92400e` |
| `color.recording` | `--q-color-recording` | `#ef4444` | `#b91c1c` |
| `color.selection` | `--q-color-selection` | `#404040` | `#d4d4d4` |
| `color.scrollbar` | `--q-color-scrollbar` | `#262626` | `#d4d4d4` |
| `color.overlay` | `--q-color-overlay` | `rgba(0,0,0,0.6)` | `rgba(0,0,0,0.3)` |
| `color.accentText` | `--q-color-accent-text` | `#171717` | `#fafafa` |

## Space

| Token | CSS variable | Value |
|---|---|---|
| `space.0` | `--q-space-0` | `0px` |
| `space.1` | `--q-space-1` | `4px` |
| `space.2` | `--q-space-2` | `8px` |
| `space.3` | `--q-space-3` | `12px` |
| `space.4` | `--q-space-4` | `16px` |
| `space.5` | `--q-space-5` | `20px` |
| `space.6` | `--q-space-6` | `24px` |
| `space.8` | `--q-space-8` | `32px` |
| `space.12` | `--q-space-12` | `48px` |
| `space.16` | `--q-space-16` | `64px` |
| `space.0.5` | `--q-space-0-5` | `2px` |

## Radius

| Token | CSS variable | Value |
|---|---|---|
| `radius.xs` | `--q-radius-xs` | `2px` |
| `radius.sm` | `--q-radius-sm` | `4px` |
| `radius.md` | `--q-radius-md` | `6px` |
| `radius.lg` | `--q-radius-lg` | `8px` |
| `radius.full` | `--q-radius-full` | `9999px` |

## Typography

| Token | CSS variable | Value |
|---|---|---|
| `font.family` | `--q-font-family` | `'Geist Pixel Square', ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace` |
| `font.weight` | `--q-font-weight` | `500` |

## Motion

Use `transition(prop, "hover" | "enter" | "exit")` instead of raw durations. Reduce Motion sets every duration to `motion.instant`.

| Token | CSS variable | Value |
|---|---|---|
| `motion.fast` | `--q-motion-fast` | `120ms` |
| `motion.normal` | `--q-motion-normal` | `180ms` |
| `motion.slow` | `--q-motion-slow` | `280ms` |
| `motion.easeOut` | `--q-motion-ease-out` | `cubic-bezier(0.16, 1, 0.3, 1)` |
| `motion.easeInOut` | `--q-motion-ease-in-out` | `cubic-bezier(0.65, 0, 0.35, 1)` |
| `motion.instant` | `--q-motion-instant` | `0ms` |
| `motion.hover` | `--q-motion-hover` | `120ms` |
| `motion.enter` | `--q-motion-enter` | `180ms` |
| `motion.exit` | `--q-motion-exit` | `120ms` |
| `motion.pulse` | `--q-motion-pulse` | `1400ms` |
| `motion.blink` | `--q-motion-blink` | `1100ms` |
| `motion.spin` | `--q-motion-spin` | `700ms` |

## Focus

Global `:focus-visible` ring for every control (it beats inline `outline` and `all: unset`). Opt out only for programmatically focused containers with `data-q-no-ring`.

| Token | CSS variable | Value |
|---|---|---|
| `focus.width` | `--q-focus-width` | `2px` |
| `focus.offset` | `--q-focus-offset` | `2px` |
| `focus.style` | `--q-focus-style` | `solid` |

## Icons

Feather icons only, through `<Icon name size>` (or `icon=` on `Button`/`IconButton`/`EmptyState`). Sizes are px squares; the stroke is shared.

| Token | CSS variable | Value |
|---|---|---|
| `icon.size.xs` | `--q-icon-size-xs` | `12` |
| `icon.size.sm` | `--q-icon-size-sm` | `14` |
| `icon.size.md` | `--q-icon-size-md` | `16` |
| `icon.size.lg` | `--q-icon-size-lg` | `20` |
| `icon.size.xl` | `--q-icon-size-xl` | `32` |
| `icon.stroke` | `--q-icon-stroke` | `2` |

## Z-index

| Token | CSS variable | Value |
|---|---|---|
| `z.base` | `--q-z-base` | `0` |
| `z.sticky` | `--q-z-sticky` | `10` |
| `z.overlay` | `--q-z-overlay` | `40` |
| `z.dialog` | `--q-z-dialog` | `50` |
| `z.toast` | `--q-z-toast` | `60` |
| `z.tooltip` | `--q-z-tooltip` | `70` |

_178 CSS variables in total._
