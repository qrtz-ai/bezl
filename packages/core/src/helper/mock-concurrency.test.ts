import { describe, expect, it } from 'vitest';
import { createMockHelper } from './mock';
import {
  encodeHostMessage,
  parseHelperLine,
  CAPTURE_SILENCE,
  HelperToHost,
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

  it('a recording whose file cannot be created is torn down at once: no pathless busy, the dictation survives', async () => {
    const { h, got, send } = rig({ failRecordingFile: true });
    send({ type: 'dictation.start', id: 'd1' });
    send({ type: 'recording.start', audioPath: '/tmp/a.wav' });
    expect(of(got, 'recording.started')).toHaveLength(0);
    expect(of(got, 'error')).toEqual([
      {
        type: 'error',
        code: 'recording.file',
        message: 'could not create the recording file',
        fatal: false,
        audioPath: '/tmp/a.wav',
      },
    ]);
    // The helper is idle for recordings again: the next start is admitted (and fails closed the same way), never
    // answered with a recording.busy that names no active file.
    send({ type: 'recording.start', audioPath: '/tmp/b.wav' });
    expect(of(got, 'error').map((e) => [e.code, e.audioPath])).toEqual([
      ['recording.file', '/tmp/a.wav'],
      ['recording.file', '/tmp/b.wav'],
    ]);
    await tick(20);
    send({ type: 'dictation.stop', id: 'd1' });
    expect(of(got, 'dictation.stopped')[0]).toMatchObject({ id: 'd1' });
    expect(of(got, 'dictation.stopped')[0]!.reason).toBeUndefined();
    h.kill();
  });

  it('N2: recording.abort tears down a wedged stop by its file; the dictation keeps running', async () => {
    const { h, got, send } = rig({ wedgeStop: {} });
    send({ type: 'dictation.start', id: 'd1' });
    send({ type: 'recording.start', audioPath: '/tmp/a.wav' });
    send({ type: 'recording.stop' });
    expect(of(got, 'recording.stopped')).toHaveLength(0); // wedged
    send({ type: 'recording.abort', audioPath: '/tmp/b.wav' }); // another file: answered, nothing changes
    send({ type: 'recording.abort', audioPath: '/tmp/a.wav' });
    expect(of(got, 'recording.aborted').map((m) => m.audioPath)).toEqual([
      '/tmp/b.wav',
      '/tmp/a.wav',
    ]);
    send({ type: 'recording.start', audioPath: '/tmp/c.wav' });
    expect(of(got, 'recording.started').map((m) => m.audioPath)).toEqual([
      '/tmp/a.wav',
      '/tmp/c.wav',
    ]);
    await tick(20);
    send({ type: 'dictation.stop', id: 'd1' });
    expect(of(got, 'dictation.stopped')[0]).toMatchObject({ id: 'd1' });
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
        audioPath: '/tmp/y.wav',
        activeAudioPath: '/tmp/x.wav',
      },
      {
        type: 'error',
        code: 'dictation.busy',
        message: 'a dictation is already running',
        fatal: false,
        id: 'b',
        activeId: 'a',
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

describe('mock helper: scripted capture.silent / capture.audible (N29)', () => {
  it('reports per source after recording.start, audible again later, nothing after stop', async () => {
    const { h, got, send } = rig({
      silence: [
        { source: 'system', afterMs: 10, audibleAfterMs: 30 },
        { source: 'mic', afterMs: 20, sinceMs: 5 },
        { source: 'mic', afterMs: 80 },
      ],
    });
    await tick(30);
    expect(of(got, 'capture.silent')).toHaveLength(0); // only while recording
    send({ type: 'recording.start', audioPath: '/tmp/s.wav' });
    await tick(50);
    expect(of(got, 'capture.silent')).toEqual([
      { type: 'capture.silent', source: 'system', sinceMs: 0 },
      { type: 'capture.silent', source: 'mic', sinceMs: 5 },
    ]);
    expect(of(got, 'capture.audible')).toEqual([
      { type: 'capture.audible', source: 'system' },
    ]);
    for (const m of [
      ...of(got, 'capture.silent'),
      ...of(got, 'capture.audible'),
    ])
      expect(HelperToHost.parse(m)).toEqual(m);
    send({ type: 'recording.stop' });
    await tick(60);
    expect(of(got, 'capture.silent')).toHaveLength(2); // the 80 ms one was cleared with the recording
    h.kill();
  });
  it('the wire shape: source is system|mic, sinceMs >= 0', () => {
    const ok = (m: unknown) => HelperToHost.safeParse(m).success;
    expect(ok({ type: 'capture.silent', source: 'mic', sinceMs: 0 })).toBe(
      true,
    );
    expect(ok({ type: 'capture.silent', source: 'mic', sinceMs: -1 })).toBe(
      false,
    );
    expect(ok({ type: 'capture.silent', source: 'camera', sinceMs: 1 })).toBe(
      false,
    );
    expect(ok({ type: 'capture.audible', source: 'system' })).toBe(true);
    expect(CAPTURE_SILENCE).toEqual({ floorRms: 0.0003, afterMs: 60_000 }); // = Swift SilenceConfig.default
  });
});

describe('mock helper: capture.fallback (H8)', () => {
  it('scripted fallbacks are sent before recording.started and parse as the wire shape', async () => {
    const { h, got, send } = rig({
      captureFallback: [
        {
          from: 'core-audio-tap',
          to: 'screencapturekit',
          reason: 'tap failed',
        },
      ],
    });
    send({ type: 'recording.start', audioPath: '/tmp/fb.wav' });
    const types = got.map((m) => m.type);
    expect(types.indexOf('capture.fallback')).toBeLessThan(
      types.indexOf('recording.started'),
    );
    expect(of(got, 'capture.fallback')).toEqual([
      {
        type: 'capture.fallback',
        from: 'core-audio-tap',
        to: 'screencapturekit',
        reason: 'tap failed',
      },
    ]);
    for (const m of of(got, 'capture.fallback'))
      expect(HelperToHost.parse(m)).toEqual(m);
    expect(
      HelperToHost.safeParse({
        type: 'capture.fallback',
        from: 'x',
        reason: 'y',
      }).success,
    ).toBe(false); // `to` is required (null = no source left)
    send({ type: 'recording.stop' });
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
      // N4: the arbiter's own degradations ride along (recording, nothing else denied → system audio only).
      expect.objectContaining({
        permission: 'microphone',
        degraded: ['no-mic'],
      }),
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
    expect(of(got, 'permission.revoked')[0]!.degraded).toEqual([]); // not recording: nothing to degrade
    expect(of(got, 'dictation.stopped')).toHaveLength(0);
    h.kill();
  });
});
