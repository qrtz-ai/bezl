// npm run screenshots: refreshes the README images in docs/screenshots from a real e2e run.
// Builds the packaged app, runs the screenshot-producing specs (under xvfb on Linux) and copies the results.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const results = path.join(root, 'test-results');
const docs = path.join(root, 'docs', 'screenshots');
const sh = (cmd, args, env = {}) => {
  const r = spawnSync(cmd, args, {
    cwd: root,
    stdio: 'inherit',
    env: { ...process.env, ...env },
  });
  if (r.status !== 0) process.exit(r.status ?? 1);
};

if (!process.argv.includes('--no-package')) sh('npm', ['run', 'package']);
const pw = [
  'playwright',
  'test',
  '-c',
  'e2e/playwright.config.ts',
  'screens',
  'states',
  'themes',
  'flows',
  'memo',
];
const linux = process.platform === 'linux' && !process.env.DISPLAY;
// screens.spec writes straight to docs/screenshots with QRTZ_DOC_SHOTS=1; the rest land in test-results.
sh(linux ? 'xvfb-run' : 'npx', linux ? ['-a', 'npx', ...pw] : pw, {
  QRTZ_DOC_SHOTS: '1',
});

/** test-results path → docs/screenshots name. */
const map = {
  'llm-error.png': 'llm-error.png',
  'helper-missing.png': 'helper-missing.png',
  'themes/light-memo-list.png': 'memo-light.png',
  'themes/light-memo-summary.png': 'meeting-summary-light.png',
  'themes/light-mini-hud.png': 'mini-hud-light.png',
  'mini-hud-notes.png': 'mini-hud.png',
  'backup-progress.png': 'backup-progress.png',
};
const missing = [];
for (const [from, to] of Object.entries(map)) {
  const src = path.join(results, from);
  if (!fs.existsSync(src)) missing.push(from);
  else fs.copyFileSync(src, path.join(docs, to));
}
if (missing.length) {
  console.error(
    `[screenshots] missing from the e2e run: ${missing.join(', ')}`,
  );
  process.exit(1);
}
console.log(
  `[screenshots] refreshed docs/screenshots (${fs.readdirSync(docs).length} files)`,
);
