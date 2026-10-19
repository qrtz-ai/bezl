// qrtz-json.mjs: every modules/<m>/qrtz.json is exactly what module.json generates.
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expected, generate, modelPackages, modules } from './qrtz-json.mjs';

const ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../..',
);
const read = (p) => JSON.parse(fs.readFileSync(path.join(ROOT, p), 'utf8'));

describe('qrtz.json is generated from module.json', () => {
  it('covers all four modules', () => {
    expect(modules()).toEqual(['ctrl', 'echo', 'nano', 'snap']);
  });
  for (const m of ['ctrl', 'echo', 'nano', 'snap']) {
    it(`${m}: qrtz.json matches, key order included`, () => {
      expect(JSON.stringify(read(`modules/${m}/qrtz.json`))).toBe(
        JSON.stringify(expected(m)),
      );
    });
  }
  it('names models by the package the runtime resolves', () => {
    const p = modelPackages();
    expect(p.get('model:parakeet-tdt-v2')).toBe('model.parakeet-tdt-v2-en');
    expect(p.get('model:nemotron-560ms')).toBe('model.nemotron');
    expect(p.get('model:laya')).toBe('model.laya');
    expect(read('modules/echo/qrtz.json').dependencies).toEqual({
      'model.parakeet-tdt-v2-en': '^1.0.0',
    });
  });
  it('fails on drift: a minApp, channel or model changed in module.json alone', () => {
    const mod = read('modules/echo/module.json');
    const have = JSON.stringify(read('modules/echo/qrtz.json'));
    const desc = read('modules/echo/qrtz.json').description;
    for (const edit of [
      (x) => (x.minApp = '0.9.0'),
      (x) => (x.protocol = 2),
      (x) => x.launch.channels.push('extra.*'),
      (x) => (x.dependencies[0].id = 'model:nemotron-560ms'),
    ]) {
      const x = structuredClone(mod);
      edit(x);
      expect(JSON.stringify(generate(x, desc))).not.toBe(have);
    }
  });
  it('refuses a model dependency that is no pinned set', () => {
    const x = read('modules/echo/module.json');
    x.dependencies[0].id = 'model:nope';
    expect(() => generate(x, '')).toThrow(/no pinned model set/);
  });
});
