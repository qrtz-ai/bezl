// QRT-297: the guard rails of .github/workflows/release.yml (Bozniak's review), checked on every CI run.
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '../..');
const wf = (f) =>
  fs.readFileSync(path.join(root, '.github/workflows', f), 'utf8');
/** The file without its comment lines (they may name what was removed). */
const code = (t) =>
  t
    .split('\n')
    .filter((l) => !/^\s*#/.test(l))
    .join('\n');
/** The text of one job: from `  <name>:` to the next two-space key. */
const job = (src, name) => {
  const m = src.match(
    new RegExp(`\\n  ${name}:\\n([\\s\\S]*?)(?=\\n  [a-z-]+:\\n|$)`),
  );
  if (!m) throw new Error(`no job ${name}`);
  return m[1];
};

describe('release.yml', () => {
  const src = code(wf('release.yml'));

  it('pins every action to a full commit SHA', () => {
    for (const f of ['release.yml']) {
      const uses = [...wf(f).matchAll(/uses:\s*(\S+)/g)].map((m) => m[1]);
      expect(uses.length).toBeGreaterThan(0);
      for (const u of uses)
        expect(u, `${f}: ${u}`).toMatch(/^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/);
    }
  });

  it('never runs on pull_request_target, and defaults to a read-only token', () => {
    expect(src).not.toContain('pull_request_target');
    expect(src).toMatch(/\npermissions:\n {2}contents: read\n/);
  });

  it('builds with no secret and a read-only token', () => {
    const b = job(src, 'build');
    expect(b).not.toContain('secrets.');
    expect(b).not.toContain('environment:');
    expect(b).toMatch(/permissions:\n\s+contents: read/);
    expect(b).not.toContain('contents: write');
  });

  it('signs only in the release environment, only for a commit on main, with contents: write only', () => {
    const s = job(src, 'sign');
    expect(s).toContain('environment: release');
    expect(s).toContain("if: needs.plan.outputs.on_main == 'true'");
    expect(s).toMatch(/permissions:\n\s+contents: write\n/);
    expect(s).toContain('scripts/ci/macos-signing.sh cleanup');
    for (const name of [
      'APPLE_CERT_P12_BASE64',
      'APPLE_CERT_PASSWORD',
      'APPLE_ID',
      'APPLE_TEAM_ID',
      'APPLE_APP_PASSWORD',
      'TAURI_SIGNING_PRIVATE_KEY',
      'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
      'QRTZ_INDEX_KEY',
      'QRTZ_TIMESTAMP_KEY',
    ])
      expect(s).toContain(`${name}: \${{ secrets.${name} }}`);
    // Secrets only through env (never interpolated into a script line).
    for (const line of src.split('\n').filter((l) => l.includes('secrets.')))
      expect(line, line).toMatch(
        /^\s+[A-Z0-9_]+: \$\{\{ secrets\.[A-Z0-9_]+ \}\}$/,
      );
  });

  it('gives the daily re-sign only the timestamp key, in its own environment', () => {
    const t = job(src, 'timestamp');
    expect(t).toContain('environment: release-timestamp');
    expect([...t.matchAll(/secrets\.(\w+)/g)].map((m) => m[1])).toEqual([
      'QRTZ_TIMESTAMP_KEY',
    ]);
  });

  it('publishes to GitHub only: no Cloudflare, R2 or rclone anywhere in the release path', () => {
    for (const f of [
      '.github/workflows/release.yml',
      'scripts/release/gh-store.sh',
      'scripts/modules/publish.sh',
      'scripts/ci/hydrate-out.sh',
      'scripts/ci/verify-live.sh',
      'scripts/app/publish-local.sh',
      'scripts/app/verify-live.sh',
      'scripts/ci/setup-secrets.sh',
    ]) {
      const t = code(fs.readFileSync(path.join(root, f), 'utf8'));
      expect(t, f).not.toMatch(
        /rclone|R2_|cloudflare|modules\.qrtz\.ai|api\.github\.com/i,
      );
    }
    for (const gone of [
      'scripts/ci/r2-env.sh',
      'scripts/app/upload.sh',
      'scripts/modules/timestamp-job.sh',
    ])
      expect(fs.existsSync(path.join(root, gone)), gone).toBe(false);
  });
});
