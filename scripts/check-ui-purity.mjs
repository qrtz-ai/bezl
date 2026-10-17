#!/usr/bin/env node
// Every visual abstraction lives in @qrtz/ui. The shell (apps/*) and feature packages compose @qrtz/ui
// components and pass data and handlers only. Outside packages/ui this fails on:
//   - style= or className= props
//   - raw intrinsic JSX elements (<div>, <span>, <button>…); only <option>/<optgroup> inside <Select> are allowed
//   - reading theme tokens to style things (useTheme, typeStyle, transition, globalCss)
//   - CSS files, CSS imports, CSS-in-JS (styled-components, emotion, css``)
//   - icon libraries (react-feather, feather-icons, any other icon set): use <Icon name> / icon= from @qrtz/ui
//   - glyph or emoji characters used as icons (■ ▶ ⌕ ✎ ⚙ …): use a Feather icon name instead
// Tests (*.test.ts[x]) are exempt: they may render fixtures.
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(
  path.dirname(new URL(import.meta.url).pathname),
  '..',
);
const SCAN = ['apps', 'packages'];
const SKIP_DIRS = new Set([
  'node_modules',
  'out',
  'dist',
  '.vite',
  'test-results',
]);
const ALLOWED_TAGS = new Set(['option', 'optgroup']);

const rules = [
  { re: /\bstyle=\{/g, msg: 'style prop (use an @qrtz/ui component)' },
  { re: /\bclassName=/g, msg: 'className prop (use an @qrtz/ui component)' },
  {
    re: /\b(useTheme|typeStyle|transition|globalCss)\s*\(/g,
    msg: 'styling from tokens (move it into @qrtz/ui)',
  },
  {
    re: /from\s+['"](styled-components|@emotion\/[\w-]+|@stitches\/[\w-]+)['"]/g,
    msg: 'CSS-in-JS library',
  },
  { re: /\bcss`/g, msg: 'css`` template' },
  { re: /import\s+['"][^'"]+\.(css|scss|sass|less)['"]/g, msg: 'CSS import' },
];

const iconRules = [
  {
    re: /from\s+['"](react-feather|feather-icons|lucide-react|react-icons(\/[\w-]+)?|@heroicons\/[\w/-]+|@radix-ui\/react-icons)['"]/g,
    msg: 'icon library import (use <Icon> / icon= from @qrtz/ui)',
  },
  {
    re: /require\(\s*['"](react-feather|feather-icons)['"]\s*\)/g,
    msg: 'icon library import (use <Icon> / icon= from @qrtz/ui)',
  },
];
// Geometric/dingbat glyphs and emoji that stand in for icons. Keyboard symbols (⌘⇧⌥⌃) and typography
// (… · — “ ”) are text, not icons, and stay allowed.
const GLYPH_RE =
  /[\u2190-\u21FF\u2300-\u2317\u2319-\u23FF\u2460-\u24FF\u25A0-\u25FF\u2600-\u27BF\u2B00-\u2BFF]|\p{Extended_Pictographic}/gu;
const KEY_SYMBOLS = new Set([
  '⌘',
  '⇧',
  '⌥',
  '⌃',
  '⌫',
  '⏎',
  '↩',
  '→',
  '←',
  '↑',
  '↓',
]);

const problems = [];
const walk = (dir) => {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (SKIP_DIRS.has(e.name) || e.name.startsWith('.')) continue;
    const p = path.join(dir, e.name);
    const rel = path.relative(ROOT, p);
    if (rel === path.join('packages', 'ui')) continue;
    if (e.isDirectory()) walk(p);
    else check(p, rel);
  }
};
const lineOf = (s, i) => s.slice(0, i).split('\n').length;
function check(p, rel) {
  if (/\.(css|scss|sass|less)$/.test(p)) {
    problems.push(`${rel}: stylesheet outside packages/ui`);
    return;
  }
  if (
    !/\.(tsx?|jsx?|mjs)$/.test(p) ||
    /\.test\.[tj]sx?$/.test(p) ||
    p.endsWith('.d.ts')
  )
    return;
  {
    const src = fs.readFileSync(p, 'utf8');
    for (const r of iconRules)
      for (const m of src.matchAll(r.re))
        problems.push(`${rel}:${lineOf(src, m.index)}: ${r.msg}`);
    if (/\.(tsx|jsx)$/.test(p) || /[\\/](shell|features)[\\/]/.test(p)) {
      const code = src.replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g, (c) =>
        c.replace(/[^\n]/g, ' '),
      );
      for (const m of code.matchAll(GLYPH_RE))
        if (!KEY_SYMBOLS.has(m[0]))
          problems.push(
            `${rel}:${lineOf(code, m.index)}: glyph "${m[0]}" used as an icon (use a Feather icon from @qrtz/ui)`,
          );
    }
  }
  // Only renderer code can style anything; main/worker/preload have no DOM.
  if (!/\.(tsx|jsx)$/.test(p) && !/[\\/](shell|renderer)[\\/.]/.test(p)) return;
  const s = fs.readFileSync(p, 'utf8');
  for (const r of rules)
    for (const m of s.matchAll(r.re))
      problems.push(`${rel}:${lineOf(s, m.index)}: ${r.msg}`);
  if (/\.(tsx|jsx)$/.test(p))
    for (const m of s.matchAll(/(?<![\w.$\])])<([a-z][a-z0-9]*)(?=[\s>/])/g))
      if (!ALLOWED_TAGS.has(m[1]))
        problems.push(
          `${rel}:${lineOf(s, m.index)}: raw <${m[1]}> element (compose @qrtz/ui components)`,
        );
}
for (const d of SCAN) walk(path.join(ROOT, d));

if (problems.length) {
  console.error(
    `check-ui-purity: ${problems.length} problem(s) outside packages/ui:\n  ${problems.join('\n  ')}`,
  );
  process.exit(1);
}
console.log('check-ui-purity: ok (no styled elements outside packages/ui)');
