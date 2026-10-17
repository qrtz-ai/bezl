#!/usr/bin/env node
// Enforces workspace dependency rules:
//   packages/core           -> (nothing internal)
//   packages/ui             -> (nothing internal)
//   packages/features/*     -> @qrtz/core, @qrtz/ui only (never other features or the shell)
//   apps/*                  -> anything
// Checks both package.json deps and actual import specifiers in source.
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { join, relative } from 'node:path';

const root = new URL('..', import.meta.url).pathname;
const rules = [
  { dir: 'packages/core', allow: [] },
  { dir: 'packages/ui', allow: [] },
  ...readdirSync(join(root, 'packages/features')).map((f) => ({
    dir: `packages/features/${f}`,
    allow: ['@qrtz/core', '@qrtz/ui'],
  })),
];

const walk = (d) =>
  readdirSync(d).flatMap((n) => {
    const p = join(d, n);
    return statSync(p).isDirectory()
      ? walk(p)
      : /\.(m?[jt]sx?)$/.test(n)
        ? [p]
        : [];
  });

const errors = [];
for (const { dir, allow } of rules) {
  const abs = join(root, dir);
  if (!existsSync(join(abs, 'package.json'))) continue;
  const pkg = JSON.parse(readFileSync(join(abs, 'package.json'), 'utf8'));
  const deps = {
    ...pkg.dependencies,
    ...pkg.devDependencies,
    ...pkg.peerDependencies,
  };
  for (const d of Object.keys(deps))
    if (d.startsWith('@qrtz/') && !allow.includes(d))
      errors.push(
        `${dir}/package.json depends on ${d} (allowed: ${allow.join(', ') || 'none'})`,
      );
  for (const file of walk(join(abs, 'src'))) {
    const src = readFileSync(file, 'utf8');
    for (const [, spec] of src.matchAll(
      /(?:from\s+|import\s*\(\s*|import\s+)['"]([^'"]+)['"]/g,
    )) {
      const rel = relative(root, file);
      if (
        spec.startsWith('@qrtz/') &&
        !allow.includes(spec.split('/').slice(0, 2).join('/'))
      )
        errors.push(`${rel} imports ${spec}`);
      if (spec.startsWith('.') && !join(file, '..', spec).startsWith(abs))
        errors.push(`${rel} reaches outside its package via ${spec}`);
    }
  }
}
if (errors.length) {
  console.error('Boundary violations:\n  ' + errors.join('\n  '));
  process.exit(1);
}
console.log(`Boundaries OK (${rules.length} packages checked)`);
