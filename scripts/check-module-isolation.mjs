#!/usr/bin/env bun
// Module isolation (Jared's rule): every package keeps its own scripts, deps and config in its own
// folder. The root package.json and Cargo.toml hold only repo-wide entries. Repo-wide safety checks
// (confinement map, egress/network tests, check-notices, verify-bundle, workspace lint rules, ci.yml)
// stay at the root and need Bozniak's sign-off to loosen.
//
// Runs in `bun run test` (check-module-isolation.test.mjs), so root lint's line is untouched.
//
// PENDING lists today's offenders while they are moved out. Shrink it; never add to it.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '..');

// Repo-wide root scripts: tooling that spans every workspace.
const ALLOWED_SCRIPTS = new Set([
  'dev:local',
  'lint',
  'lint:fix',
  'typecheck',
  'check',
  'test',
  'build:helper',
  'test:helper',
  'gen:contracts',
  'test:rust',
  'check:rust',
]);
// Repo-wide root devDependencies: the tools the root scripts above run.
const ALLOWED_DEPS = new Set([
  '@types/node',
  'oxfmt',
  'oxlint',
  'typescript',
  'vitest',
]);
// Repo-wide [workspace.dependencies]: anything at least two members use, plus internal path crates.
// A dependency only one member uses belongs in that member's Cargo.toml.

// Temporary: current offenders. Desktop entries move on foo/desktop-scripts (QRT-285);
// release:mac is Doug's call.
export const PENDING = {
  scripts: ['start', 'make', 'e2e', 'release:mac'],
  deps: [
    '@playwright/test',
    'axe-core',
    '@types/jsdom',
    'jsdom',
    '@types/react',
    '@types/react-dom',
    'react',
    'react-dom',
  ],
  cargo: ['specta'],
};

export function check(pkg, cargoToml, cargoUsers) {
  const errors = [];
  for (const s of Object.keys(pkg.scripts ?? {}))
    if (!ALLOWED_SCRIPTS.has(s) && !PENDING.scripts.includes(s))
      errors.push(
        `root package.json script "${s}" belongs in its module's package.json`,
      );
  for (const field of [
    'dependencies',
    'devDependencies',
    'optionalDependencies',
    'peerDependencies',
  ])
    for (const d of Object.keys(pkg[field] ?? {}))
      if (!ALLOWED_DEPS.has(d) && !PENDING.deps.includes(d))
        errors.push(
          `root package.json ${field} "${d}" belongs in the package that uses it`,
        );
  const section =
    cargoToml.split(/^\[workspace\.dependencies\]\s*$/m)[1]?.split(/^\[/m)[0] ??
    '';
  for (const m of section.matchAll(/^([A-Za-z0-9_-]+)\s*=\s*(.*)$/gm)) {
    const [, name, value] = m;
    if (/\bpath\s*=/.test(value)) continue;
    if ((cargoUsers[name] ?? 0) < 2 && !PENDING.cargo.includes(name))
      errors.push(
        `workspace dependency "${name}" has ${cargoUsers[name] ?? 0} user(s); pin it in that crate`,
      );
  }
  return errors;
}

function cargoUsers() {
  const toml = readFileSync(join(root, 'Cargo.toml'), 'utf8');
  const members = [
    'crates/*/Cargo.toml',
    'tools/*/Cargo.toml',
    'apps/desktop/src-tauri/Cargo.toml',
  ].flatMap((g) => [...new Bun.Glob(g).scanSync(root)]);
  const counts = {};
  for (const f of members) {
    const text = readFileSync(join(root, f), 'utf8');
    for (const m of text.matchAll(
      /^([A-Za-z0-9_-]+)(?:\.workspace\s*=\s*true|\s*=\s*\{[^}]*workspace\s*=\s*true)/gm,
    ))
      counts[m[1]] = (counts[m[1]] ?? 0) + 1;
  }
  return { toml, counts };
}

export function checkRepo() {
  const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  const { toml, counts } = cargoUsers();
  return check(pkg, toml, counts);
}

if (import.meta.main) {
  const errors = checkRepo();
  for (const e of errors) console.error(`check-module-isolation: ${e}`);
  if (errors.length) process.exit(1);
  console.log('check-module-isolation: ok');
}
