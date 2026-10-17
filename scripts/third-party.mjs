// Lists the production dependencies (what ships in the app) with their licenses: bun scripts/third-party.mjs
// Walks `dependencies` from apps/desktop through the hoisted node_modules (bunfig.toml: linker = "hoisted").
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const read = (f) => JSON.parse(fs.readFileSync(f, 'utf8'));
const workspaces = new Map();
for (const dir of ['apps', 'packages', 'packages/features'])
  for (const d of fs.readdirSync(path.join(root, dir))) {
    const f = path.join(root, dir, d, 'package.json');
    if (fs.existsSync(f)) workspaces.set(read(f).name, read(f));
  }
/** Node resolution from `from` upwards (nested copies first, then the hoisted one). */
const resolve = (name, from) => {
  for (let d = from; ; d = path.dirname(d)) {
    const f = path.join(d, 'node_modules', name, 'package.json');
    if (fs.existsSync(f)) return f;
    if (d === root || d === path.dirname(d)) return null;
  }
};
const seen = new Map();
const walk = (deps = {}, from) => {
  for (const name of Object.keys(deps)) {
    if (workspaces.has(name)) {
      walk(workspaces.get(name).dependencies, root);
      continue;
    }
    const f = resolve(name, from);
    if (!f) continue;
    const pj = read(f);
    const key = `${name}@${pj.version}`;
    if (seen.has(key)) continue;
    const license =
      pj.license ?? pj.licenses?.map?.((l) => l.type).join(' OR ') ?? 'UNKNOWN';
    seen.set(key, { name, version: pj.version, license });
    walk(pj.dependencies, path.dirname(f));
  }
};
walk(workspaces.get('@qrtz/desktop').dependencies, root);
const rows = [...seen.values()].sort((a, b) => a.name.localeCompare(b.name));
console.log(
  rows.map((r) => `| ${r.name} | ${r.version} | ${r.license} |`).join('\n'),
);
