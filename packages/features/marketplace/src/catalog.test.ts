import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import {
  compareVersions,
  infoFor,
  keptFor,
  MODEL_APPS,
  MODEL_SETS,
  listings,
  lockedDependencies,
  marketPath,
  metaLine,
  parseMarketRoute,
  sizeLine,
  statusOf,
} from './catalog';

const state = ipcSamples['packages:list'].response;
const echo = state.available.find((e) => e.name === 'echo');

describe('marketplace catalog (ADR 0161)', () => {
  it('orders versions like semver, a pre-release before its release', () => {
    expect(compareVersions('0.3.0', '0.2.9')).toBe(1);
    expect(compareVersions('1.0.0', '1.0.0')).toBe(0);
    expect(compareVersions('1.0.0-beta.1', '1.0.0')).toBe(-1);
    expect(compareVersions('0.10.0', '0.9.0')).toBe(1);
  });

  it('says installed, update or available (ADR 0169: nothing is built in)', () => {
    expect(statusOf('echo', echo, state.packages)).toBe('installed');
    expect(
      statusOf('echo', { ...echo!, version: '0.4.0' }, state.packages),
    ).toBe('update');
    // nano isn't in the lock: available, unless the host says it's installed (the mock, a dev monolith).
    expect(statusOf('nano', undefined, state.packages)).toBe('available');
    expect(statusOf('nano', undefined, state.packages, true)).toBe('installed');
    expect(statusOf('weather', undefined, [], true)).toBe('available');
  });

  it('counts dependencies in the size line', () => {
    expect(sizeLine(echo)).toBe('629 mb with dependencies');
    expect(sizeLine({ ...echo!, dependencies: [] })).toBe('9 mb');
    expect(sizeLine(undefined)).toBe('on this mac');
  });

  it("labels bundled echo with the app's version, never 'vinstalled'", () => {
    expect(metaLine('echo', undefined, '0.2.0')).toBe('v0.2.0 · built in');
    expect(metaLine('echo', undefined, null)).toBe('built in');
    expect(metaLine('echo', echo, '0.2.0')).toBe(
      `v${echo!.version} · 629 mb with dependencies`,
    );
    expect(metaLine('nano', undefined, '0.2.0')).toBe(
      'installed · on this mac',
    );
    for (const n of ['echo', 'nano', 'ctrl', 'snap'])
      expect(metaLine(n, undefined, '0.2.0')).not.toMatch(/vinstalled|vbuilt/);
  });

  it('finds what an installed extension pulls in from the lock', () => {
    const row = state.packages.find((r) => r.name === 'echo')!;
    expect(lockedDependencies(row, state.packages).map((r) => r.name)).toEqual([
      'inference.voice',
      'model.parakeet-tdt-v2-en',
    ]);
  });

  it('lists the index first and only the installed modules it lacks after', () => {
    // ADR 0169: qrtz starts empty; with no index and nothing installed there is nothing to list.
    expect(listings([])).toEqual([]);
    expect(listings([echo!]).map((l) => [l.name, !!l.entry])).toEqual([
      ['echo', true],
    ]);
    expect(
      listings([echo!], ['echo', 'snap']).map((l) => [l.name, !!l.entry]),
    ).toEqual([
      ['echo', true],
      ['snap', false],
    ]);
  });

  it('knows the first-party extensions and still lists a new one', () => {
    expect(infoFor('echo').view).toBe('talk');
    expect(infoFor('nano').view).toBe('note');
    expect(infoFor('ctrl').view).toBe('ctrl');
    expect(infoFor('snap').view).toBe('snap');
    expect(infoFor('weather')).toEqual({
      description: 'a qrtz app.',
      icon: 'package',
      view: null,
    });
    for (const n of ['echo', 'nano', 'ctrl', 'weather'])
      expect(infoFor(n).description).toBe(infoFor(n).description.toLowerCase());
  });

  it('round-trips its sub-routes and refuses junk', () => {
    for (const p of ['', 'item/echo', 'item/model.laya'])
      expect(marketPath(parseMarketRoute(p))).toBe(p);
    // The old module manager sub-route is the one list now.
    expect(parseMarketRoute('installed')).toEqual({ page: 'list' });
    expect(parseMarketRoute('item/../x')).toEqual({ page: 'list' });
    expect(parseMarketRoute('nope')).toEqual({ page: 'list' });
  });
});

describe("each app's model sets", () => {
  const root = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    '../../../..',
  );
  it("match the model: dependencies in modules/<id>/module.json (the host's sets_needed_by)", () => {
    for (const id of MODEL_APPS) {
      const m = JSON.parse(
        readFileSync(path.join(root, `modules/${id}/module.json`), 'utf8'),
      ) as { dependencies: { id: string }[] };
      expect(MODEL_SETS[id], id).toEqual(
        m.dependencies
          .map((d) => d.id)
          .filter((d) => d.startsWith('model:'))
          .map((d) => d.slice('model:'.length)),
      );
    }
  });
  it('say which sets another switched-on app keeps', () => {
    expect(keptFor('nano', ['echo', 'nano', 'ctrl'])).toEqual([
      { name: 'nemotron-560ms', keptFor: [] },
      { name: 'nemotron3-diarizer', keptFor: [] },
      { name: 'localvqe', keptFor: [] },
    ]);
    // A set two apps share stays while the other one is switched on (none share today; the rule is the host's).
    const shared = {
      ...MODEL_SETS,
      nano: [...MODEL_SETS.nano, 'parakeet-tdt-v2'],
    };
    expect(keptFor('echo', ['echo', 'nano'], shared)).toEqual([
      { name: 'parakeet-tdt-v2', keptFor: ['nano'] },
    ]);
    expect(keptFor('echo', ['echo'], shared)[0]?.keptFor).toEqual([]);
  });
});
