#!/usr/bin/env node
// Everything in @qrtz/ui is tokenized: no raw px/rem/em/hex/rgb/ms or bare numbers in style values outside
// tokens.ts. Spacing, sizes, radii, colors, type, shadows, durations and z-index all come from tokens.ts
// (or --q-* vars). Allowed raw values: 0, '100%', and the 1px hairline (written as t.border.hairline).
// Tests and stories are exempt: they may build fixtures.
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(
  path.dirname(new URL(import.meta.url).pathname),
  '..',
  'src',
);
const SKIP = (rel) =>
  rel === 'tokens.ts' || /\.test\.tsx?$/.test(rel) || rel.startsWith('fonts');
const STYLE_KEYS =
  'width|height|minWidth|minHeight|maxWidth|maxHeight|top|right|bottom|left|inset|insetInline|insetBlock|gap|rowGap|columnGap|padding\\w*|margin\\w*|border\\w*|outline\\w*|fontSize|fontWeight|lineHeight|letterSpacing|zIndex|opacity|boxShadow|transition\\w*|animation\\w*|background\\w*|color|fill|stroke|strokeWidth|flexBasis|size';
// key: 12  |  key: -3  |  key: 0.5
const NUM = new RegExp(
  `\\b(${STYLE_KEYS})\\s*:\\s*-?(\\d*\\.\\d+|[1-9]\\d*)\\b(?!\\s*[*/+-])`,
  'g',
);
// key: cond ? 12 : …  (a raw number on either side of a ternary)
const TERN = new RegExp(
  `\\b(${STYLE_KEYS})\\s*:[^,]*\\?\\s*-?([1-9]\\d*|\\d*\\.\\d+)\\s*:`,
  'g',
);
// a string literal containing a raw unit, hex or rgb color
const RAW =
  /(['"`])(?:(?!\1).)*?(?<![\w-])(-?\d*\.?\d+(px|rem|em|ms|s)\b|#[0-9a-fA-F]{3,8}\b|rgba?\()(?:(?!\1).)*?\1/g;
const ALLOW_STR = new Set([]);

const problems = [];
const walk = (dir) => {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    const rel = path.relative(ROOT, p);
    if (e.isDirectory()) walk(p);
    else if (/\.tsx?$/.test(e.name) && !SKIP(rel)) check(p, rel);
  }
};
const check = (p, rel) => {
  const lines = fs.readFileSync(p, 'utf8').split('\n');
  lines.forEach((line, i) => {
    const code = line.replace(/\/\/.*$/, '');
    if (/^\s*(\*|\/\*)/.test(line) || /tokens-allow/.test(line)) return;
    for (const m of code.matchAll(NUM))
      problems.push(`${rel}:${i + 1}: raw number ${m[0].trim()} (use a token)`);
    for (const m of code.matchAll(TERN))
      if (!/opacity/.test(m[1]))
        problems.push(
          `${rel}:${i + 1}: raw number in ${m[0].trim()} (use a token)`,
        );
    for (const m of code.matchAll(RAW)) {
      if (ALLOW_STR.has(m[0])) continue;
      if (/^['"`](import|data-|aria-)/.test(m[0])) continue;
      if (/[!=]==\s*$/.test(code.slice(0, m.index))) continue; // a comparison, not a style
      problems.push(`${rel}:${i + 1}: raw value ${m[0]} (use a token)`);
    }
  });
};
walk(ROOT);
if (problems.length) {
  console.error(problems.join('\n'));
  console.error(
    `\n${problems.length} hard-coded style value(s) in @qrtz/ui; use tokens.ts`,
  );
  process.exit(1);
}
console.log('check-tokens: @qrtz/ui is fully tokenized');
