import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ConfigSchema, defaultConfig } from './config';
import {
  GLOBAL_CONFIG_SECTIONS,
  isModuleOn,
  modulePatchError,
  pickModuleSettings,
  isViewOn,
  MODULE_IDS,
  MODULE_VIEWS,
  ModuleManifest,
  moduleOfView,
} from './modules';

const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../../..',
);
const read = (rel: string): unknown =>
  JSON.parse(readFileSync(path.join(root, rel), 'utf8'));
const bundled = MODULE_IDS.map(
  (id) => [id, read(`modules/${id}/module.json`)] as const,
);

describe('module manifests (ADR 0146)', () => {
  it('parses every bundled manifest and agrees on its views', () => {
    for (const [id, raw] of bundled) {
      const m = ModuleManifest.parse(raw);
      expect(m.id).toBe(id);
      expect(m.views).toEqual(MODULE_VIEWS[id]);
    }
  });

  it('refuses version ranges and in-process code', () => {
    const [, raw] = bundled[0];
    const base = raw as { dependencies: Record<string, unknown>[] };
    for (const version of [
      '^2.0.0',
      '~2.0.0',
      '>=2.0.0',
      '2.x',
      '*',
      '2.0',
      '2.0.0+b',
    ]) {
      const m = structuredClone(base);
      m.dependencies[0] = { ...m.dependencies[0], version };
      expect(ModuleManifest.safeParse(m).success, version).toBe(false);
    }
    for (const kind of ['dylib', 'framework']) {
      const m = structuredClone(base);
      m.dependencies[0] = { ...m.dependencies[0], kind };
      expect(ModuleManifest.safeParse(m).success, kind).toBe(false);
    }
  });

  it('gives every config section one owner: global or one module', () => {
    const owned = bundled.flatMap(
      ([, raw]) => ModuleManifest.parse(raw).settings.legacySections ?? [],
    );
    expect(new Set(owned).size).toBe(owned.length);
    const keys = Object.keys(ConfigSchema.shape).sort();
    expect([...GLOBAL_CONFIG_SECTIONS, ...owned].sort()).toEqual(keys);
  });
});

describe('global module switches', () => {
  it('default to on, so nothing changes until someone turns one off', () => {
    const cfg = defaultConfig();
    for (const id of MODULE_IDS) expect(isModuleOn(cfg, id)).toBe(true);
    for (const v of ['talk', 'note', 'ctrl', 'settings', 'gallery'])
      expect(isViewOn(cfg, v)).toBe(true);
  });

  it('control every module, and the all-modules switch wins', () => {
    const cfg = ConfigSchema.parse({ modules: { nano: { enabled: false } } });
    expect(isViewOn(cfg, 'note')).toBe(false);
    expect(isViewOn(cfg, 'talk')).toBe(true);
    const off = ConfigSchema.parse({
      modules: { enabled: false, echo: { enabled: true } },
    });
    expect(isModuleOn(off, 'echo')).toBe(false);
    expect(isViewOn(off, 'settings')).toBe(true);
    expect(moduleOfView('settings')).toBeUndefined();
  });
});

describe('module settings pages (ADR 0146 M1.4)', () => {
  const cfg = {
    theme: 'dark',
    talk: { showPill: true },
    note: { autoRecord: false },
    speech: { speakerMode: 'fast', autoUpdate: true },
    privacy: { excludedApps: ['slack'], pauseHotkey: 'X' },
  };
  it('each page sees only its own settings', () => {
    expect(pickModuleSettings(cfg, 'echo')).toEqual({
      talk: { showPill: true },
    });
    expect(pickModuleSettings(cfg, 'nano')).toEqual({
      note: { autoRecord: false },
      speech: { speakerMode: 'fast' },
      privacy: { excludedApps: ['slack'], pauseHotkey: 'X' },
    });
    expect(pickModuleSettings(cfg, 'ctrl')).toEqual({});
  });
  it('a patch outside the module is refused', () => {
    expect(modulePatchError('echo', { talk: { showPill: false } })).toBeNull();
    expect(
      modulePatchError('nano', {
        note: { autoRecord: true },
        privacy: { excludedApps: [] },
      }),
    ).toBeNull();
    expect(modulePatchError('echo', { note: {} })).toContain('note');
    expect(modulePatchError('echo', { theme: 'light' })).toContain('theme');
    expect(modulePatchError('nano', { speech: { autoUpdate: false } })).toBe(
      "nano settings can't change speech.autoUpdate",
    );
    expect(modulePatchError('ctrl', { modules: {} })).toContain('modules');
  });
});
