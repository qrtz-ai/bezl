// The release gate (verify-bundle.sh) is wired into every path that ships or publishes a qrtz.app, and refuses a
// path that isn't a bundle. The signature checks themselves need macOS and real bundles (run it on the Mac).
import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
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

  it("fails on agent dash's network helper anywhere in the bundle (ADR 0231)", () => {
    for (const rel of [
      'Contents/MacOS/qrtz-dash-net',
      'Contents/Resources/bin/qrtz-dash-net',
    ]) {
      const app = path.join(
        mkdtempSync(path.join(os.tmpdir(), 'vb-')),
        'qrtz.app',
      );
      mkdirSync(path.join(app, 'Contents/MacOS'), { recursive: true });
      mkdirSync(path.dirname(path.join(app, rel)), { recursive: true });
      writeFileSync(path.join(app, rel), '#!/bin/sh\n', { mode: 0o755 });
      const r = spawnSync('bash', [script, app], { encoding: 'utf8' });
      expect(r.status).not.toBe(0);
      expect(r.stderr).toContain(
        `${rel}: qrtz-dash-net (Agent Dash's network helper) must not ship`,
      );
    }
  });

  it('fails on a renamed copy of the helper by its compiled-in marker (ADR 0231, M5)', () => {
    const marker = read('crates/qrtz-dash-net/src/lib.rs').match(
      /BUNDLE_MARKER: &str = "([^"]+)"/,
    )[1];
    expect(read('scripts/release/verify-bundle.sh')).toContain(marker);
    const app = path.join(mkdtempSync(path.join(os.tmpdir(), 'vb-')), 'qrtz.app');
    mkdirSync(path.join(app, 'Contents/MacOS'), { recursive: true });
    mkdirSync(path.join(app, 'Contents/Resources'), { recursive: true });
    const blob = Buffer.concat([
      Buffer.from([0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0]),
      Buffer.from(marker),
      Buffer.alloc(16),
    ]);
    writeFileSync(path.join(app, 'Contents/Resources/innocent-tool'), blob, {
      mode: 0o755,
    });
    const r = spawnSync('bash', [script, app], { encoding: 'utf8' });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain(
      'Contents/Resources/innocent-tool: contains the qrtz-dash-net marker',
    );
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

  // A bundle of ad-hoc-signed stand-ins (macOS only): the entitlement allowlists and the Mach-O walk are exact.
  const mac = process.platform === 'darwin';
  const plist = (keys) =>
    `<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict>${keys.map((k) => `<key>${k}</key><true/>`).join('')}</dict></plist>`;
  const APP = [
    'com.apple.security.automation.apple-events',
    'com.apple.security.cs.allow-jit',
    'com.apple.security.device.audio-input',
    'com.apple.security.personal-information.calendars',
  ];
  function fakeApp(ents, extra) {
    const app = path.join(
      mkdtempSync(path.join(os.tmpdir(), 'vb-')),
      'qrtz.app',
    );
    const macos = path.join(app, 'Contents', 'MacOS');
    mkdirSync(macos, { recursive: true });
    for (const [name, keys] of Object.entries(ents)) {
      const bin = path.join(macos, name);
      copyFileSync('/usr/bin/true', bin);
      const args = ['--force', '--sign', '-'];
      if (keys.length) {
        const f = path.join(path.dirname(app), `${name}.plist`);
        writeFileSync(f, plist(keys));
        args.push('--entitlements', f);
      }
      const r = spawnSync('codesign', [...args, bin], { encoding: 'utf8' });
      expect(r.status, r.stderr).toBe(0);
    }
    if (extra) extra(app);
    return app;
  }
  const verify = (app) =>
    spawnSync('bash', [script, app], { encoding: 'utf8' });
  const base = () => ({ 'qrtz-app': APP, 'qrtz-echo': [APP[2]], qrtz: [] });

  it.runIf(mac)(
    'fails when qrtz-app carries an entitlement beyond its allowlist',
    () => {
      const ents = base();
      ents['qrtz-app'] = [
        ...APP,
        'com.apple.security.cs.disable-library-validation',
      ];
      const r = verify(fakeApp(ents));
      expect(r.status).toBe(1);
      expect(r.stderr).toMatch(/qrtz-app entitlements must be exactly/);
    },
  );

  it.runIf(mac)(
    'the allowlists themselves pass (only the signature checks fail on ad-hoc stand-ins)',
    () => {
      const r = verify(fakeApp(base()));
      expect(r.status).toBe(1);
      expect(r.stderr).not.toMatch(/entitlements must be exactly/);
      expect(r.stdout).toContain(
        'qrtz-echo entitlements: com.apple.security.device.audio-input',
      );
    },
  );

  it.runIf(mac)(
    'fails on get-task-allow and on an unknown Mach-O anywhere in the bundle',
    () => {
      const ents = base();
      ents['qrtz-echo'] = [APP[2], 'com.apple.security.get-task-allow'];
      const r = verify(
        fakeApp(ents, (app) => {
          const dir = path.join(app, 'Contents', 'Resources');
          mkdirSync(dir, { recursive: true });
          copyFileSync('/usr/bin/true', path.join(dir, 'sneaky'));
        }),
      );
      expect(r.status).toBe(1);
      expect(r.stderr).toContain(
        'qrtz-echo carries com.apple.security.get-task-allow',
      );
      expect(r.stderr).toContain(
        'unknown Mach-O in the bundle: Contents/Resources/sneaky',
      );
    },
  );

  it.runIf(mac)(
    'fails on a symlink in Contents that resolves outside the bundle, not on one inside',
    () => {
      const r = verify(
        fakeApp(base(), (app) => {
          const res = path.join(app, 'Contents', 'Resources');
          mkdirSync(res, { recursive: true });
          writeFileSync(path.join(res, 'notes.txt'), 'x');
          symlinkSync('notes.txt', path.join(res, 'inside'));
          symlinkSync('/etc/hosts', path.join(res, 'outside'));
          symlinkSync('../../../..', path.join(res, 'up'));
        }),
      );
      expect(r.status).toBe(1);
      expect(r.stderr).toContain(
        'symlink Contents/Resources/outside resolves outside the bundle',
      );
      expect(r.stderr).toContain(
        'symlink Contents/Resources/up resolves outside the bundle',
      );
      expect(r.stderr).not.toContain('Resources/inside');
    },
  );
});
