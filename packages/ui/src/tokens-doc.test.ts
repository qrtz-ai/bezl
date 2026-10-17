import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { tokensMarkdown } from './tokens-doc';
import { themeTokens } from './tokens';

const file = path.resolve(__dirname, '../../../docs/design-tokens.md');

/** WCAG relative luminance contrast. */
function contrast(a: string, b: string) {
  const lum = (hex: string) => {
    const [r, g, bl] = [1, 3, 5].map((i) => {
      const c = parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r! + 0.7152 * g! + 0.0722 * bl!;
  };
  const [x, y] = [lum(a), lum(b)].sort((m, n) => n - m);
  return (x! + 0.05) / (y! + 0.05);
}

describe('design tokens', () => {
  it('docs/design-tokens.md is up to date', () => {
    const md = tokensMarkdown();
    if (process.env.UPDATE_TOKENS_DOC) fs.writeFileSync(file, md);
    expect(fs.readFileSync(file, 'utf8')).toBe(md);
  });
  it('text roles meet WCAG AA on bg and surface in both palettes', () => {
    for (const mode of ['dark', 'light'] as const) {
      const c = themeTokens(mode).color;
      for (const bg of [c.bg, c.surface])
        for (const fg of [c.text, c.textMuted])
          expect(
            contrast(fg, bg),
            `${mode} ${fg} on ${bg}`,
          ).toBeGreaterThanOrEqual(4.5);
      expect(contrast(c.primaryText, c.primary)).toBeGreaterThanOrEqual(4.5);
    }
  });
});
