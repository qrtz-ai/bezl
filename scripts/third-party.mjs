// Lists the production npm dependencies (what ships in the app) with their licenses: node scripts/third-party.mjs
import { execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const tree = JSON.parse(
  execSync('npm ls --omit=dev --all --json -w apps/desktop', {
    cwd: root,
    maxBuffer: 64e6,
  }).toString(),
);
const seen = new Map();
const walk = (deps = {}) => {
  for (const [name, d] of Object.entries(deps)) {
    if (name.startsWith('@qrtz/') || !d.version) {
      walk(d.dependencies);
      continue;
    }
    const key = `${name}@${d.version}`;
    if (seen.has(key)) continue;
    let license = 'UNKNOWN';
    try {
      const pj = JSON.parse(
        fs.readFileSync(
          path.join(root, 'node_modules', name, 'package.json'),
          'utf8',
        ),
      );
      license =
        pj.license ?? pj.licenses?.map?.((l) => l.type).join(' OR ') ?? license;
    } catch {}
    seen.set(key, { name, version: d.version, license });
    walk(d.dependencies);
  }
};
for (const ws of Object.values(tree.dependencies ?? {})) walk(ws.dependencies);
const rows = [...seen.values()].sort((a, b) => a.name.localeCompare(b.name));
console.log(
  rows.map((r) => `| ${r.name} | ${r.version} | ${r.license} |`).join('\n'),
);
