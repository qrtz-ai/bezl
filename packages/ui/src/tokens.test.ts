import { describe, expect, it } from 'vitest';
import { globalCss, tokens, tokensToCssVars } from './tokens';

describe('tokens', () => {
  it('flattens into css vars', () => {
    const v = tokensToCssVars();
    expect(v['--q-color-bg']).toBe(tokens.color.bg);
    expect(v['--q-color-text-muted']).toBe('#a3a3a3');
    expect(v['--q-space-4']).toBe('16px');
    expect(v['--q-motion-ease-out']).toContain('cubic-bezier');
  });
  it('emits global css with reduced-motion support', () => {
    const css = globalCss();
    expect(css).toContain('--q-color-bg:#0a0a0a;');
    expect(css).toContain('prefers-reduced-motion');
  });
});
