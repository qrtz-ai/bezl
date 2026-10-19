import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { install, isStub, STUB_MARK, syncDevCopy } from './sidecars.mjs';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'sidecars-'));
// src-tauri/build.rs's stub, byte for byte.
const stub = (name) =>
  `#!/bin/sh\necho '${STUB_MARK} (${name}); run bun run build:helper' >&2\nexit 1\n`;
const longAgo = new Date('2001-01-01T00:00:00Z');

describe('sidecars (externalBins that `tauri dev` really runs)', () => {
  it('matches the stub src-tauri/build.rs writes', () => {
    const buildRs = fs.readFileSync(
      path.join(import.meta.dirname, '../apps/desktop/src-tauri/build.rs'),
      'utf8',
    );
    expect(buildRs).toContain(`const STUB_MARK: &str = "${STUB_MARK}";`);
    const d = tmp();
    fs.writeFileSync(path.join(d, 's'), stub('qrtz-snap'));
    expect(isStub(path.join(d, 's'))).toBe(true);
    expect(isStub(path.join(d, 'missing'))).toBe(false);
  });

  it('a changed binary gets a fresh mtime even when SwiftPM linked it long ago, so cargo reruns build.rs', () => {
    const d = tmp();
    const built = path.join(d, 'built');
    const dest = path.join(d, 'qrtz-snap-aarch64-apple-darwin');
    fs.writeFileSync(built, 'real snap');
    fs.utimesSync(built, longAgo, longAgo);
    fs.writeFileSync(dest, stub('qrtz-snap'));
    const before = Date.now() - 2000;
    expect(install(built, dest)).toBe(true);
    expect(fs.readFileSync(dest, 'utf8')).toBe('real snap');
    expect(fs.statSync(dest).mtimeMs).toBeGreaterThan(before);
    expect(fs.statSync(dest).mode & 0o777).toBe(0o755);
  });

  it('leaves an unchanged binary alone (no rebuild of the desktop crate on every start)', () => {
    const d = tmp();
    const built = path.join(d, 'built');
    const dest = path.join(d, 'dest');
    fs.writeFileSync(built, 'real helper');
    fs.writeFileSync(dest, 'real helper');
    fs.utimesSync(dest, longAgo, longAgo);
    expect(install(built, dest)).toBe(false);
    expect(fs.statSync(dest).mtimeMs).toBe(longAgo.getTime());
  });

  it('replaces by rename, never patching a copy a running qrtz has open', () => {
    const d = tmp();
    const built = path.join(d, 'built');
    const dest = path.join(d, 'dest');
    fs.writeFileSync(built, 'new');
    fs.writeFileSync(dest, 'old');
    const ino = fs.statSync(dest).ino;
    install(built, dest);
    expect(fs.statSync(dest).ino).not.toBe(ino);
    expect(fs.readdirSync(d).sort()).toEqual(['built', 'dest']);
  });

  it('syncs the real binary over a stub in target/debug, and skips a target with no debug build', () => {
    const d = tmp();
    const ext = path.join(d, 'qrtz-snap-aarch64-apple-darwin');
    fs.writeFileSync(ext, 'real snap');
    const target = path.join(d, 'target');
    expect(syncDevCopy(ext, 'qrtz-snap', target)).toBe(false);
    expect(fs.existsSync(target)).toBe(false);
    fs.mkdirSync(path.join(target, 'debug'), { recursive: true });
    fs.writeFileSync(
      path.join(target, 'debug', 'qrtz-snap'),
      stub('qrtz-snap'),
    );
    expect(syncDevCopy(ext, 'qrtz-snap', target)).toBe(true);
    expect(isStub(path.join(target, 'debug', 'qrtz-snap'))).toBe(false);
    expect(syncDevCopy(ext, 'qrtz-snap', target)).toBe(false);
  });
});
