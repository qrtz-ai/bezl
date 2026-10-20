// QRT-275: the license notices the app must ship stay in step across the repo, tauri.conf.json (what the bundle
// copies into Contents/Resources) and scripts/release/verify-bundle.sh (what a release refuses to ship without).
// bun scripts/check-notices.mjs   (part of `bun run lint`; exits 1 on any mismatch)
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const read = (p) => readFileSync(path.join(root, p), 'utf8');

// Bundle path (under Contents/Resources) -> repo source and the title its text must start with.
export const REQUIRED = {
  'THIRD_PARTY_NOTICES.md': ['THIRD_PARTY_NOTICES.md', '# Third-party notices'],
  'licenses/AGPL-3.0.txt': [
    'licenses/AGPL-3.0.txt',
    'GNU AFFERO GENERAL PUBLIC LICENSE',
  ],
  'licenses/LICENSE-EXCEPTION.md': [
    'LICENSE-EXCEPTION.md',
    '# Additional permission under AGPL-3.0 section 7',
  ],
  'licenses/GPL-3.0.txt': [
    'licenses/GPL-3.0.txt',
    'GNU GENERAL PUBLIC LICENSE',
  ],
  'licenses/Apache-2.0.txt': ['licenses/Apache-2.0.txt', 'Apache License'],
};

export function check() {
  const errors = [];
  const conf = JSON.parse(read('apps/desktop/src-tauri/tauri.conf.json'));
  const res = conf.bundle?.resources ?? {};
  const mapped = new Map(
    Object.entries(res).map(([src, dst]) => [
      dst,
      path.normalize(path.join('apps/desktop/src-tauri', src)),
    ]),
  );
  const vb = read('scripts/release/verify-bundle.sh');
  for (const [dst, [src, title]] of Object.entries(REQUIRED)) {
    if (!existsSync(path.join(root, src))) errors.push(`${src} is missing`);
    else if (
      !read(src)
        .split('\n')
        .slice(0, 5)
        .some((l) => l.includes(title))
    )
      errors.push(`${src} does not start with '${title}'`);
    if (mapped.get(dst) !== path.normalize(src))
      errors.push(
        `tauri.conf.json bundle.resources must copy ${src} to ${dst}`,
      );
    if (!vb.includes(`"${dst}|${title}"`))
      errors.push(`verify-bundle.sh does not require ${dst}`);
  }
  if (
    existsSync(path.join(root, 'licenses/AGPL-3.0.txt')) &&
    read('LICENSE') !== read('licenses/AGPL-3.0.txt')
  )
    errors.push('licenses/AGPL-3.0.txt must be identical to LICENSE');
  return errors;
}

if (import.meta.main) {
  const errors = check();
  for (const e of errors) console.error(`check-notices: ${e}`);
  if (errors.length) process.exit(1);
  console.log(
    `check-notices: ok (${Object.keys(REQUIRED).length} notices ship and verify-bundle requires them)`,
  );
}
