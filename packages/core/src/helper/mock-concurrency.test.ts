import { describe, expect, it } from 'vitest';
import { createMockHelper } from './mock';
import {
  encodeHostMessage,
  parseHelperLine,
  type HelperToHost,
  type HostToHelper,
} from './protocol';

/** H6 + H5 through the mock helper's child protocol (same arbiter as the Swift helper). */
function rig(opts: Parameters<typeof createMockHelper>[0] = {}) {
  const h = createMockHelper({ dictationStepMs: 5, heartbeatMs: 1e9, ...opts });
  const got: HelperToHost[] = [];
  h.onLine((l) => {
    const r = parseHelperLine(l);
    if (r.ok) got.push(r.msg);
  });
  const send = (m: HostToHelper) => h.write(encodeHostMessage(m));
  return { h, got, send };
}
const tick = (ms: number) => new Promise((r) => setTimeout(r, ms));
const of = <T extends HelperToHost['type']>(got: HelperToHost[], t: T) =>
  got.filter((m): m is Extract<HelperToHost, { type: T }> => m.type === t);

describe('mock helper: concurrent recording.* + dictation.* (mux)', () => {
  it('a dictation runs during a recording; both finish independently', async () => {
    const { h, got, send } = rig();
    send({ type: 'recording.start', audioPath: '/tmp/x.wav' });
    send({ type: 'dictation.start', id: 'd1' });
    await tick(40);
    expect(of(got, 'recording.started')).toHaveLength(1);
    expect(of(got, 'dictation.started')).toHaveLength(1);
    expect(got).toContainEqual({
      type: 'log',
      level: 'info',
      msg: 'mux: recording and dictation share the mic tap',
    });
    send({ type: 'dictation.stop', id: 'd1' });
    expect(of(got, 'dictation.stopped')[0]).toMatchObject({ id: 'd1' });
    expect(of(got, 'dictation.stopped')[0]!.reason).toBeUndefined();
    expect(of(got, 'recording.stopped')).toHaveLength(0);
    send({ type: 'recording.stop' });
    expect(of(got, 'recording.stopped')).toHaveLength(1);
    h.kill();
  });

  it('duplicates are ignored (single instance per kind), stops when idle are no-ops', async () => {
    const { h, got, send } = rig();
    send({ type: 'recording.stop' });
    expect(of(got, 'recording.stopped')).toHaveLength(0);
    send({ type: 'recording.start', audioPath: '/tmp/x.wav' });
    send({ type: 'recording.start', audioPath: '/tmp/y.wav' });
    send({ type: 'dictation.start', id: 'a' });
    send({ type: 'dictation.start', id: 'b' });
    expect(of(got, 'recording.started')).toHaveLength(1);
    expect(of(got, 'dictation.started').map((m) => m.id)).toEqual(['a']);
    // Ignored starts are answered, so a host waiting for started{} never hangs.
    expect(of(got, 'error')).toEqual([
      {
        type: 'error',
        code: 'recording.busy',
        message: 'a recording is already running',
        fatal: false,
      },
      {
        type: 'error',
        code: 'dictation.busy',
        message: 'a dictation is already running',
        fatal: false,
      },
    ]);
    send({ type: 'dictation.cancel', id: 'b' });
    expect(of(got, 'dictation.stopped')).toHaveLength(0);
    h.kill();
  });

  it('recording.stop with another audioPath is ignored; the matching path stops', () => {
    const { h, got, send } = rig();
    send({ type: 'recording.start', audioPath: '/tmp/b.wav' });
    send({ type: 'recording.stop', audioPath: '/tmp/a.wav' });
    expect(of(got, 'recording.stopped')).toHaveLength(0);
    send({ type: 'recording.stop', audioPath: '/tmp/b.wav' });
    expect(of(got, 'recording.stopped')).toEqual([
      expect.objectContaining({ audioPath: '/tmp/b.wav' }),
    ]);
    h.kill();
  });

  it('dictation.start is refused when the microphone is denied', () => {
    const { h, got, send } = rig({ permissions: { microphone: 'denied' } });
    send({ type: 'dictation.start', id: 'a' });
    expect(of(got, 'dictation.started')).toHaveLength(0);
    expect(of(got, 'error')[0]).toMatchObject({ code: 'dictation.mic' });
    h.kill();
  });
});

describe('mock helper: permission revoked mid-capture (H5)', () => {
  it('mic revoked during mux: permission.revoked, the dictation ends with a reason, recording continues', async () => {
    const { h, got, send } = rig({
      revoke: [{ permission: 'microphone', afterMs: 25 }],
    });
    send({ type: 'recording.start', audioPath: '/tmp/x.wav' });
    send({ type: 'dictation.start', id: 'd1' });
    await tick(60);
    expect(of(got, 'permission.revoked')).toEqual([
      expect.objectContaining({ permission: 'microphone' }),
    ]);
    const stopped = of(got, 'dictation.stopped')[0]!;
    expect(stopped).toMatchObject({ id: 'd1', reason: 'permission-revoked' });
    expect(stopped.text.length).toBeGreaterThan(0);
    send({ type: 'permissions.check' });
    expect(of(got, 'permissions.status').at(-1)!.status.microphone).toBe(
      'denied',
    );
    send({ type: 'recording.stop' });
    expect(of(got, 'recording.stopped')).toHaveLength(1);
    h.kill();
  });

  it('revocations fire once, only while capturing', async () => {
    const { h, got, send } = rig({
      revoke: [{ permission: 'accessibility', afterMs: 5 }],
    });
    await tick(20);
    expect(of(got, 'permission.revoked')).toHaveLength(0);
    send({ type: 'dictation.start', id: 'd1' });
    await tick(30);
    expect(of(got, 'permission.revoked')).toHaveLength(1);
    expect(of(got, 'dictation.stopped')).toHaveLength(0);
    h.kill();
  });
});
