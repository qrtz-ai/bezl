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
  Blocked:
    'the blurred body of ModelRequired and PermissionRequired, shown in their stories',
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

// The other direction: every exported component is rendered by the app or by another @qrtz/ui component, so the
// library holds no leftovers from removed screens (stories don't count as callers).
const NO_APP_CALLER: Record<string, string> = {
  NotchSurface:
    'web preview of the native notch panel (QrtzMacShared/NotchSurface.swift)',
  TextArea: 'form primitive, no screen needs multi-line input yet',
  Tooltip: 'overlay primitive, no screen uses one yet',
};

const sources = (dir: string): string[] =>
  readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name);
    if (e.isDirectory())
      return ['node_modules', 'dist', 'out'].includes(e.name) ? [] : sources(p);
    return /\.tsx?$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) ? [p] : [];
  });

test('every exported component has a caller outside stories', () => {
  const root = join(import.meta.dirname, '..', '..', '..');
  const app = [
    ...sources(join(root, 'apps')),
    ...sources(join(root, 'packages', 'features')),
  ]
    .map((p) => readFileSync(p, 'utf8'))
    .join('\n');
  const lib = sources(import.meta.dirname).map((p) => readFileSync(p, 'utf8'));
  const orphans = Object.entries(ui)
    .filter(([k, v]) => isComponent(k, v) && !(k in NO_APP_CALLER))
    .map(([k]) => k)
    .filter(
      (c) =>
        !new RegExp(`\\b${c}\\b`).test(app) &&
        !lib.some((s) => new RegExp(`<${c}(\\s|/|>|$)`, 'm').test(s)),
    );
  expect(orphans).toEqual([]);
  for (const k of Object.keys(NO_APP_CALLER)) expect(k in ui).toBe(true);
});
