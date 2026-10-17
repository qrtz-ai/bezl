import { describe, expect, it } from 'vitest';
import { resolveUpdateSource, shouldEnableUpdates } from './updates';

describe('auto-update policy', () => {
  it('is off in dev, on linux, and without the opt-in flag', () => {
    expect(
      shouldEnableUpdates({
        isPackaged: false,
        platform: 'darwin',
        env: { QRTZ_AUTO_UPDATE: '1' },
      }).enabled,
    ).toBe(false);
    expect(
      shouldEnableUpdates({
        isPackaged: true,
        platform: 'linux',
        env: { QRTZ_AUTO_UPDATE: '1' },
      }).enabled,
    ).toBe(false);
    expect(
      shouldEnableUpdates({ isPackaged: true, platform: 'darwin', env: {} })
        .enabled,
    ).toBe(false);
  });
  it('turns on only for signed-release opt-in on macOS', () => {
    expect(
      shouldEnableUpdates({
        isPackaged: true,
        platform: 'darwin',
        env: { QRTZ_AUTO_UPDATE: '1' },
      }).enabled,
    ).toBe(true);
  });

  it('uses a baked static feed with platform/arch placeholders', () => {
    expect(
      resolveUpdateSource({
        platform: 'darwin',
        arch: 'arm64',
        build: {
          feedUrl: 'https://updates.example.com/qrtz/{platform}/{arch}/',
        },
      }),
    ).toEqual({
      kind: 'static',
      baseUrl: 'https://updates.example.com/qrtz/darwin/arm64',
    });
    expect(
      resolveUpdateSource({
        platform: 'darwin',
        build: {
          feedUrl: 'https://github.com/o/r/releases/latest/download',
        },
      }),
    ).toEqual({
      kind: 'static',
      baseUrl: 'https://github.com/o/r/releases/latest/download',
    });
    expect(resolveUpdateSource({ platform: 'darwin' }).kind).toBe(
      'public-service',
    );
  });
  it('rejects non-https feeds and enables via the baked flag', () => {
    const bad = shouldEnableUpdates({
      isPackaged: true,
      platform: 'darwin',
      env: {},
      build: { feedUrl: 'http://insecure.example.com', autoUpdate: true },
    });
    expect(bad).toEqual({
      enabled: false,
      reason: 'update feed must be https',
    });
    expect(
      shouldEnableUpdates({
        isPackaged: true,
        platform: 'darwin',
        env: {},
        build: { feedUrl: 'https://u.example.com', autoUpdate: true },
      }).enabled,
    ).toBe(true);
  });
});
