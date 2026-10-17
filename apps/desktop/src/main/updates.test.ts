import { describe, expect, it } from 'vitest';
import { shouldEnableUpdates } from './updates';

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
});
