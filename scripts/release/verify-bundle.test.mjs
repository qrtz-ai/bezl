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
import { deflateRawSync, gzipSync } from 'node:zlib';
import path from 'node:path';

const root = path.join(import.meta.dirname, '..', '..');
const script = path.join(import.meta.dirname, 'verify-bundle.sh');
const read = (p) => readFileSync(path.join(root, p), 'utf8');

describe('verify-bundle.sh', () => {
  const verify = (app) =>
    spawnSync('bash', [script, app], { encoding: 'utf8' });
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

  // ADR 0237 (Bozniak review b): the bundled helpers and their entitlements are pinned in the gate itself.
  it('pins exactly four bundled helpers and their exact entitlements', () => {
    const s = read('scripts/release/verify-bundle.sh');
    expect(s).toContain(
      'PINNED_HELPERS="qrtz-ctrl qrtz-echo qrtz-nano qrtz-snap"',
    );
    expect(s).toContain(
      'qrtz-echo) echo "com.apple.security.device.audio-input" ;;',
    );
    expect(s).toContain(
      'qrtz-nano) echo "com.apple.security.device.audio-input com.apple.security.personal-information.calendars" ;;',
    );
    expect(s).toContain('qrtz-ctrl | qrtz-snap) echo "" ;;');
  });

  // A copy of the repo's release scripts, modules and entitlements, edited by `edit`, then run on an empty bundle:
  // the module.json-vs-pinned check runs before any signature check, so this works without codesign.
  const repoCopy = (edit) => {
    const t = mkdtempSync(path.join(os.tmpdir(), 'vb-repo-'));
    const cp = (rel) =>
      spawnSync('cp', ['-R', path.join(root, rel), path.join(t, rel)]);
    for (const d of [
      'scripts/release',
      'scripts/modules',
      'native/qrtz-helper',
    ])
      mkdirSync(path.join(t, path.dirname(d)), { recursive: true });
    cp('scripts/release');
    cp('scripts/modules');
    mkdirSync(path.join(t, 'native/qrtz-helper'), { recursive: true });
    cp('native/qrtz-helper/entitlements');
    cp('modules');
    edit(t);
    const app = path.join(t, 'qrtz.app');
    mkdirSync(path.join(app, 'Contents', 'MacOS'), { recursive: true });
    return spawnSync(
      'bash',
      [path.join(t, 'scripts/release/verify-bundle.sh'), app],
      { encoding: 'utf8' },
    );
  };
  const editJson = (t, rel, f) => {
    const p = path.join(t, rel);
    writeFileSync(p, JSON.stringify(f(JSON.parse(readFileSync(p, 'utf8')))));
  };

  it('the repo modules match the pinned table (no pin failure on an empty bundle)', () => {
    const r = repoCopy(() => {});
    expect(r.stderr).not.toContain('the release pins');
    expect(r.stderr).not.toContain('the release allows exactly');
  });

  it('fails when a module.json widens a bundled helper (ctrl gains audio-input)', () => {
    const r = repoCopy((t) => {
      editJson(t, 'modules/ctrl/module.json', (m) => ({
        ...m,
        entitlements: ['com.apple.security.device.audio-input'],
      }));
      writeFileSync(
        path.join(
          t,
          'native/qrtz-helper/entitlements/ai.qrtz.ctrl.entitlements',
        ),
        '<plist><dict><key>com.apple.security.device.audio-input</key><true/></dict></plist>',
      );
    });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(
      'qrtz-ctrl: module.json/entitlements file grant',
    );
  });

  it('fails when a fifth helper is bundled or one goes missing', () => {
    const r = repoCopy((t) => {
      editJson(t, 'modules/snap/module.json', (m) => ({
        ...m,
        launch: { ...m.launch, kind: 'package', entry: 'bin/qrtz-snap' },
      }));
    });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain('the release allows exactly');
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

  it('fails when the shipped helper carries a diagnostic CLI flag (QRT-299)', () => {
    for (const flag of ['--bench', '--echo-bench', '--echo-warmup']) {
      const app = path.join(
        mkdtempSync(path.join(os.tmpdir(), 'vb-')),
        'qrtz.app',
      );
      mkdirSync(path.join(app, 'Contents/MacOS'), { recursive: true });
      writeFileSync(
        path.join(app, 'Contents/MacOS/qrtz-echo'),
        Buffer.concat([
          Buffer.from([0xcf, 0xfa, 0xed, 0xfe]),
          Buffer.from(`\0${flag}\0`),
        ]),
        { mode: 0o755 },
      );
      const r = spawnSync('bash', [script, app], { encoding: 'utf8' });
      expect(r.status).not.toBe(0);
      expect(r.stderr).toContain(
        'Contents/MacOS/qrtz-echo contains a diagnostic CLI flag',
      );
    }
    const clean = path.join(
      mkdtempSync(path.join(os.tmpdir(), 'vb-')),
      'qrtz.app',
    );
    mkdirSync(path.join(clean, 'Contents/MacOS'), { recursive: true });
    writeFileSync(path.join(clean, 'Contents/MacOS/qrtz-echo'), '\0--stdio\0', {
      mode: 0o755,
    });
    const r = spawnSync('bash', [script, clean], { encoding: 'utf8' });
    expect(r.stdout).toContain(
      'Contents/MacOS/qrtz-echo: no diagnostic CLI flags',
    );
  });

  it('fails on a renamed copy of the helper by its compiled-in marker (ADR 0231, M5)', () => {
    const marker = read('crates/qrtz-dash-net/src/lib.rs').match(
      /BUNDLE_MARKER: &str = "([^"]+)"/,
    )[1];
    expect(read('scripts/release/verify-bundle.sh')).toContain(marker);
    const app = path.join(
      mkdtempSync(path.join(os.tmpdir(), 'vb-')),
      'qrtz.app',
    );
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

  // Bozniak M5: a compressed copy hides the marker, so archives fail by magic bytes, whatever their name.
  const helperBlob = () => {
    const marker = read('crates/qrtz-dash-net/src/lib.rs').match(
      /BUNDLE_MARKER: &str = "([^"]+)"/,
    )[1];
    return Buffer.concat([
      Buffer.from([0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1]),
      Buffer.from(marker),
      Buffer.alloc(4096, 0x41),
    ]);
  };
  const bareApp = () => {
    const app = path.join(
      mkdtempSync(path.join(os.tmpdir(), 'vb-')),
      'qrtz.app',
    );
    mkdirSync(path.join(app, 'Contents/MacOS'), { recursive: true });
    mkdirSync(path.join(app, 'Contents/Resources'), { recursive: true });
    return app;
  };

  it('fails on a gzip copy of the helper under an innocent name (M5)', () => {
    const app = bareApp();
    const gz = gzipSync(helperBlob());
    expect(gz.includes(Buffer.from('qrtz-dash-net:adr-0231'))).toBe(false);
    writeFileSync(path.join(app, 'Contents/Resources/fonts.dat'), gz);
    const r = verify(app);
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain(
      'Contents/Resources/fonts.dat: gzip archive in the bundle',
    );
    expect(r.stderr).not.toContain('contains the qrtz-dash-net marker');
  });

  it('fails on a zip (deflate) copy of the helper under an innocent name (M5)', () => {
    const app = bareApp();
    // A one-entry zip: local file header, deflated data, central directory, end record.
    const data = deflateRawSync(helperBlob());
    expect(data.includes(Buffer.from('qrtz-dash-net:adr-0231'))).toBe(false);
    const name = Buffer.from('tool');
    const lfh = Buffer.alloc(30);
    lfh.writeUInt32LE(0x04034b50, 0);
    lfh.writeUInt16LE(20, 4);
    lfh.writeUInt16LE(8, 8);
    lfh.writeUInt32LE(data.length, 18);
    lfh.writeUInt32LE(helperBlob().length, 22);
    lfh.writeUInt16LE(name.length, 26);
    const eocd = Buffer.alloc(22);
    eocd.writeUInt32LE(0x06054b50, 0);
    writeFileSync(
      path.join(app, 'Contents/Resources/readme.txt'),
      Buffer.concat([lfh, name, data, eocd]),
    );
    const r = verify(app);
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain(
      'Contents/Resources/readme.txt: zip archive in the bundle',
    );
  });

  it('fails on xz, bzip2, zstd, 7z, xar and tar by magic bytes (M5)', () => {
    const tar = Buffer.alloc(512);
    tar.write('ustar', 257);
    const cases = {
      xz: Buffer.from([0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00, 1, 2]),
      bzip2: Buffer.from('BZh91AY&SY'),
      zstd: Buffer.from([0x28, 0xb5, 0x2f, 0xfd, 0, 0]),
      '7z': Buffer.from([0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c, 0, 4]),
      xar: Buffer.from('xar!\0\x1c'),
      tar,
    };
    const app = bareApp();
    for (const [k, b] of Object.entries(cases))
      writeFileSync(path.join(app, `Contents/Resources/f-${k}`), b);
    writeFileSync(path.join(app, 'Contents/Resources/plain.txt'), 'hello\n');
    const r = verify(app);
    expect(r.status).not.toBe(0);
    for (const k of Object.keys(cases))
      expect(r.stderr).toContain(`Contents/Resources/f-${k}: ${k} archive`);
    expect(r.stderr).not.toContain('plain.txt');
  });

  it('finds Mach-Os by magic bytes anywhere and checks every identifier against the allowlist (M5)', () => {
    const s = read('scripts/release/verify-bundle.sh');
    expect(s).toContain('is not on the allowlist for this path');
    expect(s).toMatch(/feedfacf \| cefaedfe \| cffaedfe \| cafebabe/);
    const app = bareApp();
    writeFileSync(
      path.join(app, 'Contents/Resources/no-extension'),
      Buffer.from([0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1, 0, 0, 0, 0]),
    );
    const r = verify(app);
    expect(r.stderr).toContain(
      'unknown Mach-O in the bundle: Contents/Resources/no-extension',
    );
  });

  it('is called by release:mac, and publish-local.sh', () => {
    expect(read('scripts/release/sign-notarize.sh')).toContain(
      'scripts/release/verify-bundle.sh" "$APP"',
    );
    expect(read('scripts/app/publish-local.sh')).toContain(
      'scripts/release/verify-bundle.sh" "$app"',
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
  // ADR 0237: six executables; nano audio-input + calendars, ctrl and snap none.
  const base = () => ({
    'qrtz-app': APP,
    'qrtz-echo': [APP[2]],
    'qrtz-nano': [APP[2], APP[3]],
    'qrtz-ctrl': [],
    'qrtz-snap': [],
    qrtz: [],
  });

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
