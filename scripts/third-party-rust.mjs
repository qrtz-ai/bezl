// Lists the Rust crates that ship in the macOS app (the normal-dependency closure of `qrtz-desktop` for
// aarch64-apple-darwin, without dev or build dependencies and without our own workspace crates) with their licenses:
// bun scripts/third-party-rust.mjs            (a Markdown table for THIRD_PARTY_NOTICES.md)
// bun scripts/third-party-rust.mjs --summary  (crate counts per license)
// Reads `cargo metadata --offline`; never goes online.
import { execFileSync } from 'node:child_process';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const meta = JSON.parse(
  execFileSync(
    'cargo',
    [
      'metadata',
      '--format-version',
      '1',
      '--offline',
      '--locked',
      '--filter-platform',
      process.env.QRTZ_TARGET ?? 'aarch64-apple-darwin',
    ],
    { cwd: root, maxBuffer: 256 * 1024 * 1024 },
  ).toString(),
);
const pkgs = new Map(meta.packages.map((p) => [p.id, p]));
const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
const members = new Set(meta.workspace_members);
const app = meta.packages.find((p) => p.name === 'qrtz-desktop');
const seen = new Set();
const todo = [app.id];
while (todo.length) {
  const id = todo.pop();
  if (seen.has(id)) continue;
  seen.add(id);
  for (const d of nodes.get(id)?.deps ?? [])
    if (d.dep_kinds.some((k) => k.kind === null)) todo.push(d.pkg);
}
const rows = [...seen]
  .filter((id) => !members.has(id))
  .map((id) => pkgs.get(id))
  .map((p) => ({
    name: p.name,
    version: p.version,
    license:
      p.license ?? (p.license_file ? `see ${p.license_file}` : 'UNKNOWN'),
    repo: p.repository ?? '',
  }))
  .sort(
    (a, b) =>
      a.name.localeCompare(b.name) || a.version.localeCompare(b.version),
  );

if (process.argv.includes('--summary')) {
  const by = new Map();
  for (const r of rows) by.set(r.license, (by.get(r.license) ?? 0) + 1);
  for (const [l, n] of [...by].sort((a, b) => b[1] - a[1]))
    console.log(`${n}\t${l}`);
} else {
  console.log('| Crate | Version | License |');
  console.log('| --- | --- | --- |');
  for (const r of rows)
    console.log(`| ${r.name} | ${r.version} | ${r.license} |`);
}
