import { describe, expect, it } from 'vitest';
import { applyConfigPatch, defaultConfig } from './config';
import { effectiveLlmProvider, isExcludedApp } from './privacy';

describe('privacy', () => {
  it('local-only forces cloud providers off but keeps local Ollama', () => {
    const c = defaultConfig();
    expect(effectiveLlmProvider(c)).toBe('openrouter');
    const lo = applyConfigPatch(c, { privacy: { localOnly: true } });
    expect(effectiveLlmProvider(lo)).toBe('none');
    expect(
      effectiveLlmProvider(
        applyConfigPatch(lo, { note: { llmProvider: 'ollama' } }),
      ),
    ).toBe('ollama');
  });
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
