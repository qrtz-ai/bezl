import { describe, expect, it } from 'vitest';
import { applyConfigPatch, defaultConfig } from './config';
import { isExcludedApp } from './privacy';

describe('privacy', () => {
  it('matches excluded apps and platforms case-insensitively', () => {
    const c = applyConfigPatch(defaultConfig(), {
      privacy: { excludedApps: ['safari', 'Zoom'] },
    });
    expect(isExcludedApp(c, { app: 'Safari', platform: 'google-meet' })).toBe(
      true,
    );
    expect(isExcludedApp(c, { app: 'zoom.us', platform: 'zoom' })).toBe(true);
    expect(
      isExcludedApp(c, { app: 'Google Chrome', platform: 'google-meet' }),
    ).toBe(false);
  });
});
