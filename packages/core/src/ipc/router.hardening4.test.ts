/**
 * Hardening cycle 4: what a compromised or buggy renderer can send. Every malformed, oversized or unknown call is
 * refused by the router before a handler runs, and errors never carry a stack (N20).
 */
import { describe, expect, it } from 'vitest';
import { createRouter } from './router';
import { SubAppManifest } from '../subapp';

const note = SubAppManifest.parse({
  id: 'note',
  name: 'n',
  icon: 'n',
  capabilities: ['note.read', 'note.write'],
});
let ran = 0;
const router = createRouter({
  resolveSubApp: (id) => (id === 'note' ? note : undefined),
  handlers: {
    'note:pipeline': () => (++ran, null),
    'note:retry-step': () => {
      ran++;
      throw new Error('secret internal detail /Users/jared/db');
    },
    'note:live-transcript': () => (++ran, null),
    'note:get': () => (++ran, { bogus: true }) as never,
  },
});
const call = (channel: string, payload: unknown, extra: object = {}) =>
  router.dispatch({
    v: 1,
    id: 'r1',
    channel,
    version: 1,
    subApp: 'note',
    payload,
    ...extra,
  });
const big = 'x'.repeat(100_000);

describe('hardening 4: IPC contract under hostile input', () => {
  it.each([
    ['null', null],
    ['array', []],
    ['string', 'qrtz'],
    ['number', 7],
    ['no channel', { v: 1, id: 'a', version: 1, subApp: 'note' }],
    [
      'wrong protocol',
      { v: 2, id: 'a', channel: 'note:pipeline', version: 1, subApp: 'note' },
    ],
    [
      'huge id',
      { v: 1, id: big, channel: 'note:pipeline', version: 1, subApp: 'note' },
    ],
    [
      'huge channel',
      { v: 1, id: 'a', channel: big, version: 1, subApp: 'note' },
    ],
  ])('malformed envelope (%s) is E_VALIDATION', async (_n, raw) => {
    const r = await router.dispatch(raw);
    expect(r).toMatchObject({ ok: false, error: { code: 'E_VALIDATION' } });
  });

  it.each([
    '__proto__',
    'constructor',
    'toString',
    'hasOwnProperty',
    'note:nope',
  ])('unknown or prototype channel %s is E_UNKNOWN_CHANNEL', async (ch) => {
    expect(await call(ch, {})).toMatchObject({
      ok: false,
      error: { code: 'E_UNKNOWN_CHANNEL' },
    });
  });

  it('oversized meeting ids are refused before the handler runs (N20)', async () => {
    ran = 0;
    for (const [ch, payload] of [
      ['note:pipeline', { id: big }],
      ['note:retry-step', { id: big, step: 'summary' }],
      ['note:live-transcript', { meetingId: big }],
    ] as const) {
      expect(await call(ch, payload)).toMatchObject({
        ok: false,
        error: { code: 'E_VALIDATION' },
      });
    }
    expect(ran).toBe(0);
    expect(await call('note:pipeline', { id: 'a'.repeat(64) })).toMatchObject({
      ok: true,
    });
  });

  it('wrong types, unknown steps and a missing sub app never reach a handler', async () => {
    ran = 0;
    expect(await call('note:pipeline', { id: 42 })).toMatchObject({
      error: { code: 'E_VALIDATION' },
    });
    expect(
      await call('note:retry-step', { id: 'm', step: 'rm -rf' }),
    ).toMatchObject({
      error: { code: 'E_VALIDATION' },
    });
    expect(
      await call('note:pipeline', { id: 'm' }, { subApp: 'evil' }),
    ).toMatchObject({
      error: { code: 'E_FORBIDDEN' },
    });
    expect(
      await call('note:pipeline', { id: 'm' }, { version: 9 }),
    ).toMatchObject({
      error: { code: 'E_VERSION' },
    });
    expect(ran).toBe(0);
  });

  it('a handler crash is E_INTERNAL with no stack or cause; bad handler data is caught too', async () => {
    const r = await call('note:retry-step', { id: 'm', step: 'summary' });
    expect(r).toMatchObject({ ok: false, error: { code: 'E_INTERNAL' } });
    expect(Object.keys((r as { error: object }).error).sort()).toEqual([
      'code',
      'message',
      'name',
      'retryable',
    ]);
    expect(JSON.stringify(r)).not.toMatch(/\n\s+at /);
    // Accepted (cycle 4 review, §13): the message of an internal error still reaches the renderer verbatim.
    expect((r as { error: { message: string } }).error.message).toContain(
      'secret internal detail',
    );
    expect(await call('note:get', { id: 'm' })).toMatchObject({
      ok: false,
      error: { code: 'E_INTERNAL' },
    });
  });
});
