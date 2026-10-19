// The release gate (verify-bundle.sh) is wired into every path that ships or publishes a qrtz.app, and refuses a
// path that isn't a bundle. The signature checks themselves need macOS and real bundles (run it on the Mac).
import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import { readFileSync, mkdtempSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const root = path.join(import.meta.dirname, '..', '..');
const script = path.join(import.meta.dirname, 'verify-bundle.sh');
const read = (p) => readFileSync(path.join(root, p), 'utf8');

describe('verify-bundle.sh', () => {
  it('fails nonzero on something that is not an app bundle', () => {
    const dir = mkdtempSync(path.join(os.tmpdir(), 'vb-'));
    const r = spawnSync('bash', [script, dir], { encoding: 'utf8' });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain('not an app bundle');
  });

  it('pins the launch identifiers and the entitlement rules', () => {
    const s = read('scripts/release/verify-bundle.sh');
    for (const id of [
      'ai.qrtz.desktop',
      'ai.qrtz.echo',
      'ai.qrtz.cli',
      'MR26CCZ496',
    ])
      expect(s).toContain(id);
    expect(s).toContain('com.apple.security.device.audio-input');
    expect(s).toContain('stapler validate');
    expect(s).toContain('Notarized Developer ID');
  });

  it('is called by release:mac, publish-local.sh and app-release.yml', () => {
    expect(read('scripts/release/sign-notarize.sh')).toContain(
      'scripts/release/verify-bundle.sh" "$APP"',
    );
    expect(read('scripts/app/publish-local.sh')).toContain(
      'scripts/release/verify-bundle.sh" "$app"',
    );
    expect(read('.github/workflows/app-release.yml')).toContain(
      'bash scripts/release/verify-bundle.sh "$app"',
    );
  });
});
