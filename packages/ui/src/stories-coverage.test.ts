import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from 'vitest';
import * as ui from './index';

// QRT-304: every exported component appears in at least one story under packages/ui/stories.
const NOT_IN_STORIES: Record<string, string> = {
  ThemeProvider: 'wraps every story in .storybook/preview.tsx',
  ToastProvider: 'wraps every story in .storybook/preview.tsx',
  DragStrip: 'an invisible window drag region (aria-hidden), nothing to show',
  MenuEntry: 'one item of Menu and ActionMenu, shown in their stories',
};

const isComponent = (k: string, v: unknown) =>
  /^[A-Z]/.test(k) &&
  (typeof v === 'function' ||
    (typeof v === 'object' && v !== null && '$$typeof' in v));

test('every exported component appears in a story', () => {
  const dir = join(import.meta.dirname, '..', 'stories');
  const src = readdirSync(dir)
    .filter((f) => f.endsWith('.tsx'))
    .map((f) => readFileSync(join(dir, f), 'utf8'))
    .join('\n');
  const comps = Object.entries(ui)
    .filter(([k, v]) => isComponent(k, v))
    .map(([k]) => k);
  expect(comps.length).toBeGreaterThan(50);
  const missing = comps.filter(
    (c) => !(c in NOT_IN_STORIES) && !new RegExp(`<${c}[\\s/>.]`).test(src),
  );
  expect(missing).toEqual([]);
});

test('the allowlist only names real exports', () => {
  for (const k of Object.keys(NOT_IN_STORIES)) expect(k in ui).toBe(true);
});
