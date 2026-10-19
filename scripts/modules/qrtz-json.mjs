// modules/<m>/qrtz.json is generated from modules/<m>/module.json (the source of truth: the signed bundled
// manifest, ADR 0146). The one hand-written field is `description`, kept from the existing qrtz.json.
// Model dependencies: module.json names a pinned set by its dir (`model:<dir>`); the package the runtime
// resolves is the set's `package` in crates/qrtz-app/src/local_models.json or ctrl_laya_model.json.
//   bun scripts/modules/qrtz-json.mjs           write every qrtz.json
//   bun scripts/modules/qrtz-json.mjs --check   exit 1 if any qrtz.json drifts from its module.json
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../..',
);
const readJson = (p) => JSON.parse(fs.readFileSync(path.join(ROOT, p), 'utf8'));

/** `model:<dir>` → `model.<package>`, from the pinned model sets. */
export function modelPackages(root = ROOT) {
  const out = new Map();
  for (const f of [
    'crates/qrtz-app/src/local_models.json',
    'crates/qrtz-app/src/ctrl_laya_model.json',
  ]) {
    for (const s of JSON.parse(fs.readFileSync(path.join(root, f), 'utf8'))
      .sets) {
      out.set(`model:${s.dir}`, s.package);
    }
  }
  return out;
}

export function generate(module, description, packages = modelPackages()) {
  const deps = module.dependencies
    .map((d) => {
      const name = packages.get(d.id);
      if (!name)
        throw new Error(`${module.id}: ${d.id} is no pinned model set`);
      return [name, `^${d.version}`];
    })
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const l = module.launch;
  const extension = {
    entry: l.kind === 'bundled-helper' ? `bin/${l.entry}` : l.entry,
    protocol: module.protocol,
    minApp: module.minApp,
    channels: l.channels,
    views: module.views,
    capabilities: module.capabilities,
    permissions: module.permissions,
    settings: module.settings,
  };
  if (deps.length) extension.reads = deps.map(([n]) => n);
  const q = {
    name: module.name,
    version: module.version,
    kind: 'extension',
    description,
  };
  if (deps.length) q.dependencies = Object.fromEntries(deps);
  q.extension = extension;
  return q;
}

export function modules(root = ROOT) {
  return fs
    .readdirSync(path.join(root, 'modules'))
    .filter((m) => fs.existsSync(path.join(root, 'modules', m, 'module.json')))
    .sort();
}

export function expected(m, root = ROOT) {
  const mod = JSON.parse(
    fs.readFileSync(path.join(root, 'modules', m, 'module.json'), 'utf8'),
  );
  const qp = path.join(root, 'modules', m, 'qrtz.json');
  const cur = fs.existsSync(qp) ? JSON.parse(fs.readFileSync(qp, 'utf8')) : {};
  return generate(mod, cur.description ?? '', modelPackages(root));
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const check = process.argv.includes('--check');
  let bad = 0;
  for (const m of modules()) {
    const want = expected(m);
    const p = path.join(ROOT, 'modules', m, 'qrtz.json');
    const have = fs.existsSync(p) ? readJson(`modules/${m}/qrtz.json`) : null;
    if (JSON.stringify(have) === JSON.stringify(want)) continue;
    if (check) {
      console.error(
        `modules/${m}/qrtz.json drifts from module.json; run bun scripts/modules/qrtz-json.mjs`,
      );
      bad++;
    } else {
      fs.writeFileSync(p, `${JSON.stringify(want, null, 2)}\n`);
      console.log(`wrote modules/${m}/qrtz.json (format with oxfmt)`);
    }
  }
  process.exit(bad ? 1 : 0);
}
