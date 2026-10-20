import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

/**
 * Values come from tokens (QRT-301, design principles: shared values, never hard-coded).
 * Scans packages/ui/src outside tokens.ts for color literals, shadow literals, and spacing or radius
 * literals. Component geometry (a dot's width, a bar's height) is not scanned yet.
 */
const root = path.dirname(new URL(import.meta.url).pathname);
const files = (fs.readdirSync(root, { recursive: true }) as string[])
  .filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f))
  .filter((f) => path.basename(f) !== 'tokens.ts');

/** file → reason. Keep this short; every entry needs a reason. A single line can opt out with `// tokens-allow: <reason>`. */
const ALLOW: Record<string, string> = {};

const RULES: [string, RegExp][] = [
  ['color literal', /#[0-9a-fA-F]{3,8}\b|\brgba?\(/],
  ['shadow literal', /boxShadow:\s*['"`]/],
  [
    'spacing or radius literal',
    /\b(gap|rowGap|columnGap|padding\w*|margin\w*|borderRadius):\s*(?:[1-9]\d*(?:\.\d+)?\b(?!\.)|['"`][^'"`]*\b[1-9]\d*px)/,
  ],
];

function hits(): string[] {
  const out: string[] = [];
  for (const f of files) {
    if (ALLOW[f]) continue;
    const lines = fs.readFileSync(path.join(root, f), 'utf8').split('\n');
    lines.forEach((line, i) => {
      if (/\/\/ tokens-allow: \S/.test(line)) return;
      const code = line.replace(/\/\/.*$/, '');
      if (/^\s*(\*|\/\*)/.test(code)) return;
      for (const [name, re] of RULES)
        if (re.test(code)) out.push(`${f}:${i + 1} ${name}: ${line.trim()}`);
    });
  }
  return out;
}

describe('@qrtz/ui uses tokens, not literals', () => {
  it('has no color, shadow, spacing or radius literals outside tokens.ts', () => {
    expect(hits()).toEqual([]);
  });
});
