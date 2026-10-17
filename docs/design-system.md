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

- **Light theme decision:** qrtz.ai is dark-only, so dark stays the default brand theme. Light and System are opt-in under Settings → Appearance. They use the same neutral roles, with status colors darkened to pass AA. The gallery shows a Dark/Light preview (`ThemeScope`), and axe scans both.
- The full token reference is in [design-tokens.md](design-tokens.md). It is generated, and a unit test fails when it drifts or when text contrast drops below AA.
- **Motion:** use `transition(prop, 'hover' | 'enter' | 'exit')`. Reduce Motion collapses every duration to `motion.instant`.
- **Focus:** one global `:focus-visible` ring comes from the `focus.*` tokens. `data-q-no-ring` opts out containers that get focus programmatically.
