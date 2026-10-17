import { describe, expect, it } from 'vitest';
import {
  applyConfigPatch,
  defaultConfig,
  loadConfig,
  migrateLegacyConfig,
} from './config';
import { MemorySecretStore, envNameFor } from './secrets';

describe('config', () => {
  it('fills defaults', () => {
    expect(loadConfig(undefined).speech).toEqual({
      speakerMode: 'fast',
      autoUpdate: true,
    });
    // ADR 0046: the old model-named setting is dropped on load, the rest of `note` survives.
    const old = loadConfig({
      note: { liveAsr: 'parakeet-unified-320ms', autoRecord: true },
    });
    expect('liveAsr' in old.note).toBe(false);
    expect(old.note.autoRecord).toBe(true);
  });
  it('N29: the no-audio warning is on by default and can be turned off', () => {
    expect(defaultConfig().note.notifyNoAudio).toBe(true);
    expect(
      loadConfig({ note: { notifyNoAudio: false } }).note.notifyNoAudio,
    ).toBe(false);
    expect(
      applyConfigPatch(defaultConfig(), { note: { notifyNoAudio: false } }).note
        .notifyNoAudio,
    ).toBe(false);
  });
  it('keeps valid keys when one is invalid', () => {
    const c = loadConfig({ theme: 'neon', logLevel: 'debug' });
    expect(c.theme).toBe('dark');
    expect(c.logLevel).toBe('debug');
  });
  it('patches deeply and rejects invalid writes and prototype pollution', () => {
    const c = applyConfigPatch(defaultConfig(), {
      note: { defaultTemplate: 'sales' },
    });
    expect(c.note.defaultTemplate).toBe('sales');
    expect(c.note.autoDetectMeetings).toBe(true);
    expect(() =>
      applyConfigPatch(c, { note: { defaultTemplate: 'evil' } }),
    ).toThrow();
    applyConfigPatch(c, JSON.parse('{"__proto__":{"polluted":1}}'));
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
  });
  it('migrates the pre-rename memo settings to note (note keys win)', () => {
    const legacy = {
      theme: 'light',
      memo: { autoRecord: true, retainAudioDays: 30, detectPrompt: false },
    };
    const c = loadConfig(legacy);
    expect(c.theme).toBe('light');
    expect(c.note.autoRecord).toBe(true);
    expect(c.note.retainAudioDays).toBe(30);
    expect(c.note.detectPrompt).toBe(false);
    expect('memo' in c).toBe(false);
    const both = loadConfig({
      memo: { autoRecord: true, floatingHud: false },
      note: { autoRecord: false },
    });
    expect(both.note.autoRecord).toBe(false);
    expect(both.note.floatingHud).toBe(false);
    expect(migrateLegacyConfig({ note: {} }).migrated).toBe(false);
    expect(migrateLegacyConfig(legacy).migrated).toBe(true);
  });

  it('ADR 0076: an old config with the cloud settings (deepgram/elevenlabs, openrouter, local-only) loads without them', () => {
    const old = {
      talk: {
        engine: 'deepgram',
        autoFailover: false,
        showPill: false,
        pushKey: 'right-option',
      },
      note: {
        llmProvider: 'openrouter',
        llmModel: 'anthropic/claude-haiku-5.5',
        autoSummarize: false,
      },
      speech: { cloud: 'elevenlabs', speakerMode: 'accurate' },
      privacy: { localOnly: true, excludedApps: ['Slack'] },
    };
    const c = loadConfig(old);
    const has = (o: object, k: string) => k in o;
    expect(has(c.talk, 'engine') || has(c.talk, 'autoFailover')).toBe(false);
    expect(has(c.note, 'llmProvider') || has(c.note, 'llmModel')).toBe(false);
    expect(has(c.speech, 'cloud') || has(c.privacy, 'localOnly')).toBe(false);
    expect(c.talk.showPill).toBe(false);
    expect(c.talk.pushKey).toBe('right-option');
    expect(c.note.autoSummarize).toBe(false);
    expect(c.speech.speakerMode).toBe('accurate');
    expect(c.privacy.excludedApps).toEqual(['Slack']);
    expect(migrateLegacyConfig(old).migrated).toBe(false);
  });

  it('an old config with the removed talk ai cleanup settings loads without them', () => {
    const c = loadConfig({
      talk: {
        aiCleanup: false,
        cleanupModel: 'anthropic/claude-haiku-5.5',
        cleanupTimeoutMs: 900,
        autoCleanup: false,
      },
    });
    expect(c.talk.autoCleanup).toBe(false);
    for (const k of ['aiCleanup', 'cleanupModel', 'cleanupTimeoutMs'])
      expect(k in c.talk).toBe(false);
  });

  it('an old config with the removed search model settings loads without them', () => {
    const c = loadConfig({
      note: {
        searchModelDownload: false,
        backgroundIndexing: false,
        autoSummarize: false,
      },
    });
    expect(c.note.autoSummarize).toBe(false);
    for (const k of ['searchModelDownload', 'backgroundIndexing'])
      expect(k in c.note).toBe(false);
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
