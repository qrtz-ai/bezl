// Prints the CHANGELOG.md section for one version (app-release.yml uses it for the update notes in latest.json).
//   bun scripts/release-notes.mjs v0.2.0
// Matches a heading "## 0.2.0" or "## v0.2.0", optionally followed by a date or title ("## 0.2.0 (2026-10-18)",
// "## v0.2.0: offline"), up to the next "## " heading. Exits 1 if there is no such section or it is empty.
import fs from 'node:fs';
import path from 'node:path';

const arg = process.argv[2];
if (!arg) {
  console.error('usage: bun scripts/release-notes.mjs <version|tag>');
  process.exit(2);
}
const version = arg.replace(/^v/, '');
const esc = version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const heading = new RegExp(`^## v?${esc}(?![\\w.-])`);
const lines = fs
  .readFileSync(path.join(import.meta.dirname, '..', 'CHANGELOG.md'), 'utf8')
  .split('\n');
const start = lines.findIndex((l) => heading.test(l));
if (start < 0) {
  console.error(`[release-notes] no "## ${version}" section in CHANGELOG.md`);
  process.exit(1);
}
let end = lines.findIndex((l, i) => i > start && l.startsWith('## '));
if (end < 0) end = lines.length;
const body = lines
  .slice(start + 1, end)
  .join('\n')
  .trim();
if (!body) {
  console.error(`[release-notes] the "## ${version}" section is empty`);
  process.exit(1);
}
process.stdout.write(`${body}\n`);
