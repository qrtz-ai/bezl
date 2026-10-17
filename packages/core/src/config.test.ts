import { describe, expect, it } from 'vitest';
import { applyConfigPatch, defaultConfig, loadConfig } from './config';
import { MemorySecretStore, envNameFor } from './secrets';

describe('config', () => {
  it('fills defaults', () => {
    expect(loadConfig(undefined).memo.liveAsr).toBe('parakeet-unified-320ms');
  });
  it('keeps valid keys when one is invalid', () => {
    const c = loadConfig({ theme: 'neon', logLevel: 'debug' });
    expect(c.theme).toBe('dark');
    expect(c.logLevel).toBe('debug');
  });
  it('patches deeply and rejects invalid writes and prototype pollution', () => {
    const c = applyConfigPatch(defaultConfig(), {
      memo: { llmProvider: 'anthropic' },
    });
    expect(c.memo.llmProvider).toBe('anthropic');
    expect(c.memo.autoDetectMeetings).toBe(true);
    expect(() =>
      applyConfigPatch(c, { memo: { llmProvider: 'evil' } }),
    ).toThrow();
    applyConfigPatch(c, JSON.parse('{"__proto__":{"polluted":1}}'));
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
  });
});

describe('secrets', () => {
  it('memory store lists names only', async () => {
    const s = new MemorySecretStore();
    await s.set('llm.openai', 'sk-1');
    expect(await s.list()).toEqual(['llm.openai']);
    await s.set('llm.openai', null);
    expect(await s.get('llm.openai')).toBeUndefined();
    expect(envNameFor('llm.openai')).toBe('QRTZ_SECRET_LLM_OPENAI');
  });
});
