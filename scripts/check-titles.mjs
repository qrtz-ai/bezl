#!/usr/bin/env node
// Brand rule: every title in qrtz is fully lowercase. @qrtz/ui lowercases titles at render time (lowerTitle); this
// keeps the *source* strings lowercase too so accessible names, window titles and the tray match.
// Checked: <Heading>literal</Heading> (unless preserveCase), title="…" on SettingsSection/Section/Dialog/
// ConfirmDialog/EmptyState, sub-app manifest names, BrowserWindow/Notification titles in main, tray menu labels.
// Symbols like ⌘ ⇧ are fine (they have no case). Body text and user content are not checked.
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(
  path.dirname(new URL(import.meta.url).pathname),
  '..',
);
const SKIP = new Set([
  'node_modules',
  'out',
  'dist',
  'dist-types',
  '.vite',
  'test-results',
]);
const UPPER = /\p{Lu}/u;

export function titleProblems(src, rel) {
  const out = [];
  const line = (i) => src.slice(0, i).split('\n').length;
  const add = (i, s, what) => {
    // Keyboard shortcuts (⌘K, ⌘⇧R) keep their key letter.
    if (UPPER.test(s.replace(/[⌘⇧⌥⌃]+\p{Lu}?/gu, '')))
      out.push(
        `${rel}:${line(i)}: ${what} "${s.trim()}" has uppercase letters`,
      );
  };
  for (const m of src.matchAll(/<Heading\b([^>]*)>([^<{]+)<\/Heading>/g))
    if (!/preserveCase/.test(m[1])) add(m.index, m[2], 'heading');
  for (const m of src.matchAll(
    /<(SettingsSection|Section|ConfirmDialog|Dialog|EmptyState)\b[^>]*?\btitle="([^"]+)"/gs,
  ))
    add(m.index, m[2], `${m[1]} title`);
  // Tabs: { id: '…', label: '…' } literal arrays passed to <Tabs>.
  for (const m of src.matchAll(/<Tabs\b[\s\S]*?tabs=\{\[([\s\S]*?)\]\}/g))
    for (const l of m[1].matchAll(/label:\s*'([^']+)'/g))
      add(m.index, l[1], 'tab label');
  if (/[\\/]features[\\/][^\\/]+[\\/]src[\\/]manifest\.ts$/.test(rel))
    for (const m of src.matchAll(/\bname:\s*'([^']+)'/g))
      add(m.index, m[1], 'sub-app name');
  if (/[\\/]main[\\/]/.test(rel)) {
    for (const m of src.matchAll(/\btitle:\s*'([^']+)'/g))
      add(m.index, m[1], 'window/notification title');
    if (/tray/.test(rel))
      for (const m of src.matchAll(
        /\blabel:\s*'([^']+)'|['`]((?:[A-Z][a-z]+)(?: [^'`$]*)?)['`]/g,
      ))
        add(m.index, m[1] ?? m[2], 'tray label');
  }
  return out;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const problems = [];
  const walk = (dir) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      if (SKIP.has(e.name) || e.name.startsWith('.')) continue;
      const p = path.join(dir, e.name);
      if (e.isDirectory()) walk(p);
      else if (
        /\.(tsx?)$/.test(p) &&
        !/\.test\.tsx?$/.test(p) &&
        !p.endsWith('.d.ts')
      )
        problems.push(
          ...titleProblems(fs.readFileSync(p, 'utf8'), path.relative(ROOT, p)),
        );
    }
  };
  for (const d of ['apps', 'packages']) walk(path.join(ROOT, d));
  if (problems.length) {
    console.error(
      `check-titles: ${problems.length} title(s) not lowercase:\n  ${problems.join('\n  ')}`,
    );
    process.exit(1);
  }
  console.log('check-titles: ok (all titles lowercase)');
}
