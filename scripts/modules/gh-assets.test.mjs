// ADR 0218: the flat GitHub release asset names map one to one onto the module store layout.
import { describe, expect, it } from 'vitest';
import { assetName, layoutPath, uploadListArgs } from './gh-assets.mjs';

const h = 'ab'.repeat(32);
const paths = [
  'v1/root/1.root.json',
  'v1/root/1.root.json.sig',
  'v1/root/12.root.json',
  'v1/timestamp.json',
  'v1/timestamp.json.sig',
  `v1/index/${h}.json`,
  `v1/index/${h}.json.sig`,
  `v1/blobs/sha256/${h}`,
];

describe('gh-assets names', () => {
  it('round-trips every layout path', () => {
    for (const p of paths) expect(layoutPath(assetName(p))).toBe(p);
    expect(assetName(`v1/blobs/sha256/${h}`)).toBe(`v1.blobs.sha256.${h}`);
    expect(new Set(paths.map(assetName)).size).toBe(paths.length);
  });
  it('refuses anything else', () => {
    for (const n of [
      'v1.app.latest.json',
      'v1.root.0.root.json',
      `v1.blobs.sha256.${h}.sig`,
      'qrtz_0.3.0.dmg',
      '../x',
    ])
      expect(layoutPath(n)).toBeNull();
    expect(() => assetName('v1/app/latest.json')).toThrow();
  });
});

describe('gh-assets upload list (QRT-343)', () => {
  it('narrows to the timestamp pair only when the release exists', () => {
    expect(uploadListArgs('/o', true, true)).toEqual([
      'upload-list',
      '--out',
      '/o',
      '--timestamp-only',
    ]);
  });
  it('lists the whole seeded out dir for the first upload, timestamp mode too', () => {
    expect(uploadListArgs('/o', true, false)).toEqual([
      'upload-list',
      '--out',
      '/o',
    ]);
    expect(uploadListArgs('/o', false, false)).toEqual([
      'upload-list',
      '--out',
      '/o',
    ]);
    expect(uploadListArgs('/o', false, true)).toEqual([
      'upload-list',
      '--out',
      '/o',
    ]);
  });
});
