# Design system notes (inspiration: https://qrtz.ai)

The site HTML was fetched on 2026-10-16. Fetching the CSS bundle was blocked by tooling, so
the tokens are derived from the site's Tailwind/shadcn class names and its visual language:

- **Dark only** (`<html class="dark">`), `bg-background text-foreground`, `antialiased`.
  It's a neutral near-black and off-white (shadcn neutral dark palette).
- **Type:**
  - A geometric sans, Geist-style (`__className_*` from next/font).
  - **Mono** for product names (`font-mono font-semibold`).
  - Big hero `text-5xl/6xl font-bold` with a blinking `_` cursor.
  - Muted body copy `text-muted-foreground`.
- **Surfaces:** `border border-border rounded-lg p-6` cards. Thin 1px borders and no
  heavy shadows.
- **Badges:** `px-2 py-0.5 text-xs font-semibold bg-primary/10 text-primary border
  border-primary/20 rounded`.
- **Motion:** an underline that scales in from the left (`h-0.5 bg-primary origin-left`)
  on hover, and a blinking caret. The feel is fast and minimal, using about 150–200 ms
  ease-out transitions.
- **Spacing:** generous; sections use `py-16 px-4` and stacks use `space-y-6`.

## Themes, motion and focus (iteration 14)

- **Light theme decision:** qrtz.ai is dark-only, so dark stays the default brand theme. Light and System are opt-in under Settings → Appearance. They use the same neutral roles, with status colors darkened to pass AA. Storybook's foundations show a Dark/Light preview (`ThemeScope`), and axe scans both themes in e2e.
- The full token reference is in [design-tokens.md](design-tokens.md). It is generated, and a unit test fails when it drifts or when text contrast drops below AA.
- **Motion:** use `transition(prop, 'hover' | 'enter' | 'exit')`. Reduce Motion collapses every duration to `motion.instant`.
- **Focus:** one global `:focus-visible` ring comes from the `focus.*` tokens. `data-q-no-ring` opts out containers that get focus programmatically.

## One font, one scale (redesign)

Everything renders in **Geist Pixel Square** (OFL-1.1, `GeistPixel-Square.woff2`, bundled from the `geist` npm package into
`packages/ui/src/fonts`). It has one weight, so every weight token is 500 and `font-synthesis: none`
stops fake bold. Sizes sit on the font's pixel grid (13.158px = one device pixel per font pixel on
Retina), so nothing renders below 13.158px. Hierarchy comes from size, color, case and tracking.
See Storybook's **foundations → type scale** and `docs/design-tokens.md`.

Use `Text variant=…`, `Heading level=…`, `Label`, `Code`/`Mono` and `Link`.
`scripts/check-typography.mjs` fails on raw font-size/family/weight/line-height/letter-spacing.

## Every visual abstraction lives in @qrtz/ui

The shell (`apps/desktop/src/shell`) and feature packages compose `@qrtz/ui` and pass data and
handlers only. `scripts/check-ui-purity.mjs` (part of `bun run lint`) fails outside `packages/ui` on:

- `style=` or `className=` props
- raw intrinsic JSX elements (`<div>`, `<span>`, `<button>`…); only `<option>`/`<optgroup>` are allowed
- styling from tokens (`useTheme()`, `typeStyle()`, `transition()`, `globalCss()`)
- stylesheets, CSS imports and CSS-in-JS
- icon library imports (`react-feather`, `feather-icons`, `lucide-react`, …) and glyph/emoji characters
  used as icons (■ ▶ ⌕ ✎ ⚙ …); keyboard symbols (⌘ ⇧ ⌥) and typography (… ·) are text and stay allowed

Layout uses `Stack` (token gaps and padding only, semantic `as` for form/ul/li/nav/section).
App and note pieces: `AppFrame`, `SideNav`, `SplitView`, `Page`, `Toolbar`, `IconButton`, `Chip`,
`ListRow`/`MeetingRow`, `SettingsSection`/`SettingRow`, `TranscriptRow`/`TranscriptList`,
`NotesApp`, `SummaryBlock`, `RecordingBar`, `RecordDot`,
`LevelMeter`, `StreamingTranscript`/`PipelineSteps`/`CaptureStatus`, `Banner`, `AudioPlayer`, `MoreMenu`, `ConfirmDialog`,
`CommandPalette`, `PermissionList`, `MiniHudFrame`/`NoDrag`/`StatusLines`.

The library and the app stay in step both ways (`packages/ui/src/stories-coverage.test.ts`): every exported
component appears in a Storybook story (`bun run storybook` in `packages/ui`), and every exported component is
rendered by the app or by another `@qrtz/ui` component, so nothing is left over from removed screens.
Storybook is the one component catalog; there is no in-app gallery.

## Icons (Feather)

Every icon is a [Feather](https://feathericons.com) icon (`react-feather` 2.0.10, MIT) rendered by one
component, `<Icon name size tone fill label>` in `@qrtz/ui`. Names are a curated semantic set
(`ICONS` in `packages/ui/src/Icon.tsx`: `search`, `ask`, `stop`, `play`, `pause`, `more`, `edit`,
`refresh`, `todo`, `mic`, `settings`, `grid`, `circle`, …); add a Feather glyph there before using it.
Sizes come from `tokens.icon.size` (`xs` 12, `sm` 14, `md` 16, `lg` 20, `xl` 32 px) and every icon uses
`tokens.icon.stroke` (2). Icons are decorative (`aria-hidden`) unless given a `label`; `IconButton`
always carries an accessible `label`. `Button icon=`, `IconButton icon=`, `EmptyState icon=` and
`CommandPalette` results take an icon name; sub-app manifests name theirs (`icon: 'mic'`). Storybook's
**foundations → icons (feather)** lists every name, the size scale and the tones.

## Layout

All main navigation lives in a left sidebar (`SideNav` in `AppFrame`): one icon + lowercase label row
per view (the modules, then marketplace), with `settings` pinned to the
sidebar's foot. There is no bottom bar. Content fills the rest of the window. Pages have no title or header
row: the first section starts at the top. An app's opts open from the ⋯ at the end of its sidebar row (shown on
hover, on focus and on the active row); echo's dictate button and on/off switch are its first row. Note is two panes: the meeting list (Record button, search toggle) on the left, the selected meeting on the right
(summary first, then notes and transcript one tab away). No explanatory copy: labels, glyphs and layout carry the UI.
