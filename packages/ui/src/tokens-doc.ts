import { themeTokens, tokens, tokensToCssVars } from './tokens';

/** Markdown reference for every token (docs/design-tokens.md is generated from this and checked in tests). */
export function tokensMarkdown(): string {
  const dark = themeTokens('dark');
  const light = themeTokens('light');
  const vars = tokensToCssVars(tokens);
  const varOf = (path: string) =>
    `\`--q-${path.replace(/\./g, '-').replace(/[A-Z]/g, (m) => '-' + m.toLowerCase())}\``;
  const out: string[] = [
    '# @qrtz/ui design tokens',
    '',
    '<!-- Generated from packages/ui/src/tokens.ts by tokens-doc.test.ts (UPDATE_TOKENS_DOC=1 npm test). Do not edit by hand. -->',
    '',
    'Use tokens through `useTheme()` in components, or the CSS custom properties that `ThemeProvider` injects.',
    'Dark is the qrtz.ai brand default. Light is an opt-in palette with the same roles; choose it in Settings → Appearance (Dark / Light / System).',
    '',
    '## Color',
    '',
    '| Token | CSS variable | Dark | Light |',
    '|---|---|---|---|',
  ];
  for (const k of Object.keys(dark.color) as (keyof typeof dark.color)[])
    out.push(
      `| \`color.${k}\` | ${varOf(`color.${k}`)} | \`${dark.color[k]}\` | \`${light.color[k]}\` |`,
    );
  const table = (
    title: string,
    group: Record<string, unknown>,
    prefix: string,
    note?: string,
  ) => {
    out.push('', `## ${title}`, '');
    if (note) out.push(note, '');
    out.push('| Token | CSS variable | Value |', '|---|---|---|');
    const walk = (obj: Record<string, unknown>, p: string) => {
      for (const [k, v] of Object.entries(obj)) {
        if (typeof v === 'object' && v)
          walk(v as Record<string, unknown>, `${p}.${k}`);
        else
          out.push(
            `| \`${p}.${k}\` | ${varOf(`${p}.${k}`)} | \`${String(v).replace(/\|/g, '\\|')}\` |`,
          );
      }
    };
    walk(group, prefix);
  };
  table('Space', tokens.space, 'space');
  table('Radius', tokens.radius, 'radius');
  table('Typography', tokens.font, 'font');
  table(
    'Motion',
    tokens.motion,
    'motion',
    'Use `transition(prop, "hover" | "enter" | "exit")` instead of raw durations. Reduce Motion sets every duration to `motion.instant`.',
  );
  table(
    'Focus',
    tokens.focus,
    'focus',
    'Global `:focus-visible` ring for every control (it beats inline `outline` and `all: unset`). Opt out only for programmatically focused containers with `data-q-no-ring`.',
  );
  table(
    'Icons',
    tokens.icon,
    'icon',
    'Feather icons only, through `<Icon name size>` (or `icon=` on `Button`/`IconButton`/`EmptyState`). Sizes are px squares; the stroke is shared.',
  );
  table('Z-index', tokens.z, 'z');
  out.push('', `_${Object.keys(vars).length} CSS variables in total._`, '');
  return out.join('\n');
}
