import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  EventBus,
  MemorySecretStore,
  applyConfigPatch,
  createMockHelper,
  defaultConfig,
  encodeHostMessage,
  nullLogger,
  parseHelperLine,
  type Config,
  type HostToHelper,
  type TalkView,
} from '@qrtz/core';
import { TalkService, type TalkDeps } from './talk-service';
import { fakeCleanupLlm, fakeCloudWs } from './talk-dev';

const until = async (f: () => boolean, ms = 3000) => {
  const t0 = Date.now();
  while (!f()) {
    if (Date.now() - t0 > ms) throw new Error('timeout');
    await new Promise((r) => setTimeout(r, 5));
  }
};

function setup(
  o: {
    insert?: 'ax' | 'paste' | 'fail';
    revokeMicAfterMs?: number;
    cfg?: unknown;
    llm?: TalkDeps['llm'];
    ws?: TalkDeps['ws'];
    secrets?: Record<string, string>;
  } = {},
) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-talk-'));
  const helper = createMockHelper({
    ...(o.insert ? { insert: o.insert } : {}),
    ...(o.revokeMicAfterMs !== undefined
      ? {
          revoke: [
            {
              permission: 'microphone' as const,
              afterMs: o.revokeMicAfterMs,
            },
          ],
        }
      : {}),
    dictationStepMs: 5,
    heartbeatMs: 1e9,
  });
  const bus = new EventBus();
  const states: TalkView[] = [];
  bus.on('talk:state', (v) => states.push(v));
  const cfg: Config = applyConfigPatch(defaultConfig(), o.cfg ?? {});
  const secrets = new MemorySecretStore();
  for (const [k, v] of Object.entries(o.secrets ?? {})) void secrets.set(k, v);
  const sent: string[] = [];
  const sentMsgs: HostToHelper[] = [];
  const clipboard: string[] = [];
  const svc = new TalkService({
    send: (m) => {
      sent.push(m.type);
      sentMsgs.push(m);
      helper.write(encodeHostMessage(m));
    },
    bus,
    log: nullLogger,
    config: () => cfg,
    secrets,
    storePath: path.join(dir, 'talk.json'),
    ws: o.ws ?? fakeCloudWs('echo'),
    llm: o.llm ?? (async () => null),
    writeClipboard: (t) => clipboard.push(t),
    timeouts: {
      started: 500,
      stopped: 500,
      insert: 300,
      cloudStall: 100,
      cloudFinal: 200,
    },
  });
  helper.onLine((l) => {
    const r = parseHelperLine(l);
    if (r.ok) svc.handle(r.msg);
  });
  return { svc, states, sent, sentMsgs, clipboard, dir, helper };
}

describe('TalkService', () => {
  it('push-to-talk: streams partials, cleans up with rules, inserts via AX, records latency + history', async () => {
    const { svc, states, sent, dir } = setup();
    await svc.configure();
    expect(sent).toContain('dictation.configure');
    svc.handle({ type: 'hotkey', action: 'down' });
    await until(() => svc.state().partial.split(' ').length >= 6);
    expect(svc.state()).toMatchObject({
      phase: 'listening',
      engine: 'on-device',
    });
    svc.handle({ type: 'hotkey', action: 'up' });
    await until(() => svc.state().phase === 'idle' && !!svc.state().last);
    const e = svc.state().last!;
    expect(e).toMatchObject({
      cleanup: 'rules',
      engine: 'on-device',
      insert: 'ax-selected-text',
      inserted: true,
      app: 'Mock Editor',
    });
    expect(e.text.startsWith('So I think')).toBe(true);
    expect(e.latency.firstPartialMs).toBeGreaterThanOrEqual(0);
    expect(e.latency.insertedMs).toBeGreaterThanOrEqual(0);
    expect(states.map((s) => s.phase)).toEqual(
      expect.arrayContaining([
        'listening',
        'finalizing',
        'cleaning',
        'inserting',
        'idle',
      ]),
    );
    expect(svc.history()).toHaveLength(1);
    // Persisted and reloaded.
    expect(
      JSON.parse(fs.readFileSync(path.join(dir, 'talk.json'), 'utf8')).history,
    ).toHaveLength(1);
  });

  it('H5: microphone revoked mid-dictation stops gracefully, keeps what was heard, then shows why', async () => {
    const { svc, states } = setup({ revokeMicAfterMs: 30 });
    await svc.configure();
    svc.handle({ type: 'hotkey', action: 'down' });
    await until(() => svc.state().phase === 'error');
    expect(svc.state().error).toMatch(/microphone access was turned off/);
    const e = svc.state().last!;
    expect(e.text.length).toBeGreaterThan(0);
    expect(e.inserted).toBe(true);
    expect(states.map((s) => s.phase)).toEqual(
      expect.arrayContaining(['listening', 'finalizing', 'inserting', 'error']),
    );
    // The next dictation is refused by the helper (mic denied) and surfaces as an error, not a hang.
    svc.handle({ type: 'hotkey', action: 'up' });
    await svc.start();
    await until(() => /microphone access denied/.test(svc.state().error ?? ''));
  });

  it('H6: a start the helper answers with dictation.busy fails at once instead of hanging', async () => {
    const { svc, sentMsgs } = setup();
    await svc.configure();
    const p = svc.start();
    const id = svc.state().id!;
    svc.handle({
      type: 'error',
      code: 'dictation.busy',
      message: 'a dictation is already running',
      fatal: false,
      id,
      activeId: 'orphan',
    });
    await p;
    expect(svc.state()).toMatchObject({
      phase: 'error',
      error: 'another dictation is still running',
    });
    // A stop now returns immediately (nothing to wait for).
    await svc.stop();
    expect(svc.state().phase).toBe('error');
    // The orphan is cancelled by its own id; the refused start is never cancelled (the helper never ran it).
    const cancels = sentMsgs.filter((m) => m.type === 'dictation.cancel');
    expect(cancels).toEqual([{ type: 'dictation.cancel', id: 'orphan' }]);
  });

  it('busy re-review: a helper still running d1 after a host reload no longer locks dictation out', async () => {
    const { svc, sentMsgs, helper } = setup();
    await svc.configure();
    // The previous host started d1 and went away; this TalkService has never heard of it.
    helper.write(encodeHostMessage({ type: 'dictation.start', id: 'd1' }));
    await svc.start();
    const d2 = sentMsgs.find((m) => m.type === 'dictation.start')!;
    expect(svc.state()).toMatchObject({
      phase: 'error',
      error: 'another dictation is still running',
    });
    const cancels = sentMsgs.filter((m) => m.type === 'dictation.cancel');
    expect(cancels).toEqual([{ type: 'dictation.cancel', id: 'd1' }]);
    expect(cancels).not.toContainEqual({
      type: 'dictation.cancel',
      id: 'id' in d2 ? d2.id : '',
    });
    // The retry gets the helper: it listens and hears.
    await svc.start();
    expect(svc.state().phase).toBe('listening');
    await until(() => svc.state().partial.split(' ').length >= 3);
    await svc.stop();
    await until(() => !!svc.state().last);
    expect(svc.state().last?.text).toBeTruthy();
  });

  it('busy re-review: a busy for another start, or naming our own id as active, never fails the dictation', async () => {
    const { svc, sentMsgs } = setup();
    await svc.configure();
    await svc.start();
    const id = svc.state().id!;
    const busy = (o: { id?: string; activeId?: string }) =>
      svc.handle({
        type: 'error',
        code: 'dictation.busy',
        message: 'a dictation is already running',
        fatal: false,
        ...o,
      });
    busy({ id: 'stale', activeId: id }); // a late answer to an older start
    busy({}); // an old helper with no ids
    busy({ id, activeId: id }); // a duplicate start{id} while id runs
    expect(svc.state()).toMatchObject({ phase: 'listening', id });
    expect(sentMsgs.filter((m) => m.type === 'dictation.cancel')).toEqual([]);
    await until(() => svc.state().partial.split(' ').length >= 3);
    await svc.stop();
    await until(() => !!svc.state().last);
  });

  it('dictionary terms are applied; AI cleanup is used when fast', async () => {
    const { svc } = setup({ llm: async () => fakeCleanupLlm('fake') });
    svc.setDictionary([{ term: 'qrtz', aliases: ['cue arts'] }]);
    await svc.start();
    await until(() => svc.state().partial.includes('friday'));
    await svc.stop();
    await until(() => !!svc.state().last);
    expect(svc.state().last).toMatchObject({
      cleanup: 'llm',
      text: 'I think we should ship qrtz talk on friday.',
    });
  });

  it('cleanup timeout falls back to rules', async () => {
    const { svc } = setup({
      llm: async () => fakeCleanupLlm('hang'),
      cfg: { talk: { cleanupTimeoutMs: 100 } },
    });
    await svc.start();
    await until(() => svc.state().partial.length > 0);
    await svc.stop();
    await until(() => !!svc.state().last, 2000);
    expect(svc.state().last).toMatchObject({ cleanup: 'rules' });
    expect(svc.state().last!.latency.cleanupMs).toBeGreaterThanOrEqual(90);
  });

  it('insertion fallback: paste is reported; total failure leaves the text on the clipboard', async () => {
    const p = setup({ insert: 'paste' });
    await p.svc.start();
    await until(() => p.svc.state().partial.length > 0);
    await p.svc.stop();
    await until(() => !!p.svc.state().last);
    expect(p.svc.state().last).toMatchObject({
      insert: 'paste',
      inserted: true,
    });

    const f = setup({ insert: 'fail' });
    await f.svc.start();
    await until(() => f.svc.state().partial.length > 0);
    await f.svc.stop();
    await until(() => !!f.svc.state().last);
    expect(f.svc.state().last).toMatchObject({
      insert: 'none',
      inserted: false,
    });
    expect(f.svc.state().last!.error).toContain('copied to clipboard');
    expect(f.clipboard).toEqual([f.svc.state().last!.text]);
  });

  it('cloud engine is primary when a key exists, and fails over to on-device when the socket dies', async () => {
    const ok = setup({
      secrets: { 'asr.deepgram': 'k' },
      ws: fakeCloudWs('echo'),
    });
    await ok.svc.configure();
    expect(ok.svc.resolveEngine()).toBe('deepgram');
    await ok.svc.start();
    expect(ok.svc.state().engine).toBe('deepgram');
    await until(() => ok.svc.state().partial.length > 0);
    await ok.svc.stop();
    await until(() => !!ok.svc.state().last);
    expect(ok.svc.state().last).toMatchObject({
      engine: 'deepgram',
      failover: false,
    });

    const bad = setup({
      secrets: { 'asr.deepgram': 'k' },
      ws: fakeCloudWs('fail'),
    });
    await bad.svc.configure();
    await bad.svc.start();
    await until(() => bad.svc.state().failover === true);
    expect(bad.svc.state().engine).toBe('on-device');
    await bad.svc.stop();
    await until(() => !!bad.svc.state().last);
    expect(bad.svc.state().last).toMatchObject({
      engine: 'on-device',
      failover: true,
      inserted: true,
    });

    const silent = setup({
      secrets: { 'asr.deepgram': 'k' },
      ws: fakeCloudWs('silent'),
    });
    await silent.svc.configure();
    await silent.svc.start();
    await until(() => silent.svc.state().failover === true, 2000);
  });

  it('local-only privacy and missing keys force on-device; cancel discards', async () => {
    const lo = setup({
      secrets: { 'asr.deepgram': 'k' },
      cfg: { privacy: { localOnly: true } },
    });
    await lo.svc.configure();
    expect(lo.svc.resolveEngine()).toBe('on-device');
    const none = setup({ cfg: { talk: { engine: 'elevenlabs' } } });
    await none.svc.configure();
    expect(none.svc.resolveEngine()).toBe('on-device');
    await none.svc.start();
    await none.svc.cancel();
    expect(none.svc.state().phase).toBe('idle');
    expect(none.svc.history()).toHaveLength(0);
  });

  it('autoInsert/saveHistory off are respected; disabled talk ignores the hotkey', async () => {
    const { svc, sent } = setup({
      cfg: { talk: { autoInsert: false, saveHistory: false } },
    });
    await svc.start();
    await until(() => svc.state().partial.length > 0);
    await svc.stop();
    await until(() => !!svc.state().last);
    expect(sent).not.toContain('insert.text');
    expect(svc.history()).toHaveLength(0);
    const off = setup({ cfg: { talk: { enabled: false } } });
    off.svc.handle({ type: 'hotkey', action: 'down' });
    expect(off.svc.state().phase).toBe('idle');
  });
});
