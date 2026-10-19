import { describe, expect, it } from 'vitest';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import {
  compareVersions,
  infoFor,
  listings,
  lockedDependencies,
  marketPath,
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
      description: 'a qrtz extension.',
      icon: 'package',
      view: null,
    });
    for (const n of ['echo', 'nano', 'ctrl', 'weather'])
      expect(infoFor(n).description).toBe(infoFor(n).description.toLowerCase());
  });

  it('round-trips its sub-routes and refuses junk', () => {
    for (const p of ['', 'installed', 'item/echo', 'item/model.laya'])
      expect(marketPath(parseMarketRoute(p))).toBe(p);
    expect(parseMarketRoute('item/../x')).toEqual({ page: 'browse' });
    expect(parseMarketRoute('nope')).toEqual({ page: 'browse' });
  });
});
