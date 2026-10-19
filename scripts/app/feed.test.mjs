// scripts/app/feed.mjs: the latest.json the app parses (ADR 0181), with the test fixtures self_update.rs uses.
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { archivePath, feed, keyFrom } from './feed.mjs';

const fx = path.join(
  import.meta.dirname,
  '../../crates/qrtz-app/tests/fixtures/app-update',
);

describe('scripts/app/feed.mjs', () => {
  it('writes the versioned archive url, signature, sha256 and size', () => {
    const f = feed({
      version: '0.3.0',
      archive: path.join(fx, 'artifact.tar.gz'),
      sig: path.join(fx, 'good-0.3.0.sig'),
      notes: 'faster',
      date: new Date('2026-10-19T05:00:00.123Z'),
    });
    const p = f.platforms['darwin-aarch64'];
    expect(f.version).toBe('0.3.0');
    expect(f.pub_date).toBe('2026-10-19T05:00:00Z');
    expect(p.url).toBe(
      'https://modules.qrtz.ai/v1/app/0.3.0/qrtz_0.3.0_aarch64.app.tar.gz',
    );
    expect(archivePath('0.3.0')).toBe(
      'v1/app/0.3.0/qrtz_0.3.0_aarch64.app.tar.gz',
    );
    expect(p.size).toBe(fs.statSync(path.join(fx, 'artifact.tar.gz')).size);
    expect(p.sha256).toMatch(/^[0-9a-f]{64}$/);
    expect(p.signature).toBe(
      fs.readFileSync(path.join(fx, 'good-0.3.0.sig'), 'utf8').trim(),
    );
    expect(() => feed({ version: '0.3.0-beta', archive: '', sig: '' })).toThrow(
      /plain/,
    );
  });

  it('reads the committed key file: the TODO is no key, one key is', () => {
    const todo = fs.readFileSync(
      path.join(
        import.meta.dirname,
        '../../crates/qrtz-app/src/app_update_key.pub',
      ),
      'utf8',
    );
    const test = fs.readFileSync(path.join(fx, 'test.pub'), 'utf8').trim();
    if (keyFrom(todo) === null) expect(todo).toContain('TODO(ADR 0181)');
    expect(keyFrom(`# comment\n${test}\n`)).toBe(test);
    expect(keyFrom(`${test}\n${test}\n`)).toBeNull();
    expect(keyFrom('not a key')).toBeNull();
  });
});
