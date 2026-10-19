// Typography guard (part of `bun run lint`): outside packages/ui no raw font-family / font-size / font-weight /
// line-height / letter-spacing (inline styles, CSS strings, or Text/Heading size/weight props). Use
// <Text variant>, <Heading level>, <Label>, <Code>, <Link> or typeStyle() instead.
//   bun scripts/check-typography.mjs           → fail on violations
//   bun scripts/check-typography.mjs --audit   → distinct raw values per property, whole repo (incl. packages/ui)
// It also holds the one-font rule: Geist Pixel Square is the only font. packages/ui/src/fonts bundles only
// GeistPixel-Square.woff2 (plus its OFL), tokens.font.family leads with 'Geist Pixel Square', and no file in
// apps/ or packages/ names another font face (an @font-face, a font file, or another Geist Pixel style).
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const SKIP =
  /(node_modules|[\\/]out[\\/]|[\\/]\.vite[\\/]|\.test\.tsx?$|tokens-doc\.ts$)/;
const files = [];
const walk = (d) => {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = path.join(d, e.name);
    if (SKIP.test(p)) continue;
    if (e.isDirectory()) walk(p);
    else if (/\.(tsx?|css|html)$/.test(e.name)) files.push(p);
  }
};
walk(path.join(root, 'apps'));
walk(path.join(root, 'packages'));

const PROPS = {
  'font-family': [
    /\bfontFamily\s*:\s*([^,}\n]+)/g,
    /font-family\s*:\s*([^;}"'`\n]+)/g,
  ],
  'font-size': [
    /\bfontSize\s*:\s*([^,}\n]+)/g,
    /font-size\s*:\s*([^;}"'`\n]+)/g,
    /<(?:Text|Heading)\b[^>]*?\bsize=\{?["']?([\w.-]+)/g,
  ],
  'font-weight': [
    /\bfontWeight\s*:\s*([^,}\n]+)/g,
    /font-weight\s*:\s*([^;}"'`\n]+)/g,
    /<(?:Text|Heading)\b[^>]*?\bweight=\{?["']?([\w.-]+)/g,
  ],
  'line-height': [
    /\blineHeight\s*:\s*([^,}\n]+)/g,
    /line-height\s*:\s*([^;}"'`\n]+)/g,
  ],
  'letter-spacing': [
    /\bletterSpacing\s*:\s*([^,}\n]+)/g,
    /letter-spacing\s*:\s*([^;}"'`\n]+)/g,
  ],
  font: [/[{;\s"'`]font\s*:\s*([^;}"'`\n]+)/g],
};
const distinct = Object.fromEntries(
  Object.keys(PROPS).map((k) => [k, new Set()]),
);
const violations = [];
for (const f of files) {
  const rel = path.relative(root, f);
  const inUi = rel.startsWith(`packages${path.sep}ui${path.sep}`);
  const src = fs.readFileSync(f, 'utf8');
  for (const [prop, res] of Object.entries(PROPS))
    for (const re of res)
      for (const m of src.matchAll(re)) {
        const v = m[1].trim().replace(/[,;]$/, '');
        if (!v || v === '{' || /^'?inherit'?$/.test(v)) continue; // `font: inherit` resets are fine
        distinct[prop].add(v);
        if (!inUi)
          violations.push(
            `${rel}:${src.slice(0, m.index).split('\n').length}  ${prop}: ${v}`,
          );
      }
}
// One font: Geist Pixel Square.
const FONT = 'Geist Pixel Square';
const FONT_FILE = 'GeistPixel-Square.woff2';
const fontDir = path.join(root, 'packages', 'ui', 'src', 'fonts');
for (const name of fs.readdirSync(fontDir))
  if (name !== FONT_FILE && !/^OFL-.*\.txt$/.test(name))
    violations.push(
      `packages/ui/src/fonts/${name}  only ${FONT_FILE} is bundled`,
    );
if (!fs.existsSync(path.join(fontDir, FONT_FILE)))
  violations.push(`packages/ui/src/fonts/${FONT_FILE}  missing`);
const tokensSrc = fs.readFileSync(
  path.join(root, 'packages', 'ui', 'src', 'tokens.ts'),
  'utf8',
);
const fam = /\bfamily:\s*["'`](.*?)["'`],?\n/s.exec(tokensSrc)?.[1] ?? '';
if (!fam.startsWith(`'${FONT}'`))
  violations.push(
    `packages/ui/src/tokens.ts  font.family must lead with '${FONT}' (got ${fam || 'nothing'})`,
  );
const OTHER_STYLE = /Geist ?Pixel[- ]?(Circle|Grid|Line|Triangle)\b/gi;
const FONT_FILE_REF = /[\w.-]+\.(?:woff2?|ttf|otf)\b/g;
const FACE = /@font-face\s*\{\s*font-family\s*:\s*(['"]?)([^;'"}]+)\1/g;
for (const f of files) {
  const rel = path.relative(root, f);
  const src = fs.readFileSync(f, 'utf8');
  const at = (i) => `${rel}:${src.slice(0, i).split('\n').length}`;
  for (const m of src.matchAll(OTHER_STYLE))
    violations.push(`${at(m.index)}  font: ${m[0]} (the only font is ${FONT})`);
  for (const m of src.matchAll(FONT_FILE_REF))
    if (m[0] !== FONT_FILE)
      violations.push(`${at(m.index)}  font file: ${m[0]} (only ${FONT_FILE})`);
  for (const m of src.matchAll(FACE))
    if (m[2].trim() !== FONT)
      violations.push(`${at(m.index)}  @font-face: ${m[2]} (only ${FONT})`);
}

if (process.argv.includes('--audit')) {
  let total = 0;
  for (const [k, s] of Object.entries(distinct)) {
    total += s.size;
    console.log(
      `${k.padEnd(15)} ${String(s.size).padStart(3)}  ${[...s].sort().join(' | ')}`,
    );
  }
  console.log(
    `${'TOTAL'.padEnd(15)} ${String(total).padStart(3)}  (raw typography outside packages/ui: ${violations.length})`,
  );
  process.exit(0);
}
if (violations.length) {
  console.error(
    `Typography violations (${violations.length}). The only font is Geist Pixel Square; outside packages/ui use <Text variant>, <Heading>, <Label>, <Code>, <Link> or typeStyle():\n  ${violations.join('\n  ')}`,
  );
  process.exit(1);
}
console.log('Typography OK');
