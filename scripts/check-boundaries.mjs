#!/usr/bin/env node
// Enforces workspace dependency rules (see PLAN.md "Boundary rules").
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const NODE_OR_ELECTRON = (s) =>
  s === 'electron' ||
  s.startsWith('node:') ||
  /^(fs|path|child_process|os|net|http|https|crypto)$/.test(s);

/** Returns rule objects for a repo root. */
export function rulesFor(root) {
  const featuresDir = join(root, 'packages/features');
  const features = existsSync(featuresDir) ? readdirSync(featuresDir) : [];
  return [
    {
      dir: 'packages/core',
      allowInternal: [],
      banned: (s) =>
        NODE_OR_ELECTRON(s) || s === 'react-dom' || s.startsWith('react-dom/'),
    },
    { dir: 'packages/ui', allowInternal: [], banned: NODE_OR_ELECTRON },
    ...features.map((f) => ({
      dir: `packages/features/${f}`,
      allowInternal: ['@qrtz/core', '@qrtz/ui'],
      banned: NODE_OR_ELECTRON,
    })),
    // Renderer code inside the shell must stay sandbox-safe.
    {
      dir: 'apps/desktop',
      src: ['src/shell', 'src/renderer.tsx'],
      allowInternal: null,
      banned: NODE_OR_ELECTRON,
      skipPkg: true,
    },
  ];
}

const walk = (p) => {
  if (!existsSync(p)) return [];
  if (!statSync(p).isDirectory()) return [p];
  return readdirSync(p).flatMap((n) => walk(join(p, n)));
};
const isTest = (f) => /\.test\.[cm]?[jt]sx?$/.test(f);
const IMPORT_RE =
  /(?:^|[^.\w])(?:from\s+|import\s*\(\s*|import\s+|require\s*\(\s*)['"]([^'"]+)['"]/g;

export function checkBoundaries(root) {
  const errors = [];
  const rules = rulesFor(root);
  for (const rule of rules) {
    const abs = join(root, rule.dir);
    if (!existsSync(join(abs, 'package.json'))) continue;
    if (!rule.skipPkg) {
      const pkg = JSON.parse(readFileSync(join(abs, 'package.json'), 'utf8'));
      const deps = { ...pkg.dependencies, ...pkg.peerDependencies };
      for (const d of Object.keys(deps)) {
        if (d.startsWith('@qrtz/') && !rule.allowInternal.includes(d))
          errors.push(`${rule.dir}/package.json depends on ${d}`);
        if (rule.banned(d))
          errors.push(`${rule.dir}/package.json depends on banned ${d}`);
      }
    }
    const files = (rule.src ?? ['src'])
      .flatMap((s) => walk(join(abs, s)))
      .filter((f) => /\.[cm]?[jt]sx?$/.test(f) && !isTest(f));
    for (const file of files) {
      const src = readFileSync(file, 'utf8');
      const rel = relative(root, file);
      for (const [, spec] of src.matchAll(IMPORT_RE)) {
        const pkgName = spec.startsWith('@')
          ? spec.split('/').slice(0, 2).join('/')
          : spec;
        if (
          rule.allowInternal &&
          spec.startsWith('@qrtz/') &&
          !rule.allowInternal.includes(pkgName)
        )
          errors.push(`${rel} imports ${spec}`);
        if (rule.banned(spec))
          errors.push(`${rel} imports banned module ${spec}`);
        if (
          !rule.src &&
          spec.startsWith('.') &&
          !join(file, '..', spec).startsWith(abs)
        )
          errors.push(`${rel} reaches outside its package via ${spec}`);
      }
    }
  }
  return { errors, checked: rules.length };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = fileURLToPath(new URL('..', import.meta.url));
  const { errors, checked } = checkBoundaries(root);
  if (errors.length) {
    console.error('Boundary violations:\n  ' + errors.join('\n  '));
    process.exit(1);
  }
  console.log(`Boundaries OK (${checked} rule sets)`);
}
