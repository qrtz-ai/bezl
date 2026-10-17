import {
  HELPER_PROTOCOL_VERSION,
  HostToHelper,
  type HelperToHost,
} from './protocol';
import type { ChildHandle, Clock } from './supervisor';
import { systemClock } from './supervisor';
import {
  FIXTURE_DURATION_MS,
  STANDUP_FIXTURE,
  fixtureActiveSpeakers,
  fixtureAsrEvents,
} from '../memo/fixtures';

export interface MockHelperOptions {
  clock?: Clock;
  heartbeatMs?: number;
  os?: string;
  /** Play a scripted meeting (AX detection + streaming ASR). */
  scenario?: 'standup';
  /** Time scale for the scenario (0.05 = 20× faster than real time). */
  speed?: number;
  /** Delay before the meeting is "detected" (ms, real). */
  detectAfterMs?: number;
  /** Override reported TCC states (fault injection: denied permissions). */
  permissions?: Partial<
    Record<
      'accessibility' | 'screen-recording' | 'microphone',
      'granted' | 'denied' | 'unsupported'
    >
  >;
}

/**
 * In-process TypeScript stand-in for the Swift helper. Speaks the same JSON-lines protocol.
 * Used in tests, on non-macOS hosts, and for the Playwright e2e (scenario: 'standup').
 */
export function createMockHelper(
  opts: MockHelperOptions = {},
): ChildHandle & { crash(code?: number): void } {
  const clock = opts.clock ?? systemClock;
  const speed = opts.speed ?? 1;
  const lineCbs: ((l: string) => void)[] = [];
  const exitCbs: ((c: number | null, s: string | null) => void)[] = [];
  let alive = true;
  let hb: unknown = null;
  let scriptTimers: unknown[] = [];
  let audioPath = '';
  const granted = opts.scenario ? 'granted' : 'unsupported';
  const emit = (m: HelperToHost) => {
    if (alive) for (const cb of lineCbs) cb(JSON.stringify(m));
  };
  const at = (ms: number, fn: () => void) =>
    scriptTimers.push(clock.setTimeout(fn, ms));
  const clearScript = () => {
    for (const t of scriptTimers) clock.clearTimeout(t);
    scriptTimers = [];
  };
  const beat = () => {
    emit({ type: 'heartbeat', ts: clock.now() });
    hb = clock.setTimeout(beat, opts.heartbeatMs ?? 5000);
  };
  const die = (code: number | null, signal: string | null) => {
    if (!alive) return;
    alive = false;
    if (hb) clock.clearTimeout(hb);
    clearScript();
    for (const cb of exitCbs) cb(code, signal);
  };
  const permissions = (): HelperToHost => ({
    type: 'permissions.status',
    status: {
      accessibility: granted,
      'screen-recording': granted,
      microphone: granted,
      ...opts.permissions,
    },
  });

  clock.setTimeout(() => {
    emit({
      type: 'hello',
      protocol: HELPER_PROTOCOL_VERSION,
      helperVersion: 'mock',
      os: opts.os ?? 'mock',
      features: [
        'mock',
        ...(opts.scenario ? [`scenario:${opts.scenario}`] : []),
      ],
    });
    beat();
    if (opts.scenario === 'standup') {
      clock.setTimeout(() => {
        emit({
          type: 'ax.participants',
          names: [...STANDUP_FIXTURE.participants],
        });
        emit({ type: 'meeting.detected', ...STANDUP_FIXTURE.meeting });
      }, opts.detectAfterMs ?? 300);
    }
  }, 0);

  let paused = false;
  const startScript = () => {
    const asr = fixtureAsrEvents();
    // Spread partials evenly between utterance start and its final.
    const finalsAt = new Map(
      STANDUP_FIXTURE.lines.map(([, , end], i) => [`u${i}`, end]),
    );
    const counts = new Map<string, number>();
    for (const e of asr)
      counts.set(e.utteranceId, (counts.get(e.utteranceId) ?? 0) + 1);
    const seen = new Map<string, number>();
    for (const e of asr) {
      const n = (seen.get(e.utteranceId) ?? 0) + 1;
      seen.set(e.utteranceId, n);
      const end = finalsAt.get(e.utteranceId) ?? e.startMs;
      const t =
        e.kind === 'final'
          ? end
          : e.startMs +
            ((end - e.startMs) * n) / (counts.get(e.utteranceId)! + 0.5);
      at(t * speed, () => {
        if (!paused) emit({ type: 'asr', event: e });
      });
    }
    for (const a of fixtureActiveSpeakers())
      at(a.atMs * speed, () => emit({ type: 'ax.activeSpeaker', ...a }));
    for (let t = 0; t < FIXTURE_DURATION_MS; t += 500)
      at(t * speed, () =>
        emit({
          type: 'audio.level',
          rms: paused ? 0 : 0.2 + 0.6 * Math.abs(Math.sin(t / 700)),
        }),
      );
  };

  return {
    onLine: (cb) => void lineCbs.push(cb),
    onExit: (cb) => void exitCbs.push(cb),
    write(line) {
      let parsed;
      try {
        parsed = HostToHelper.safeParse(JSON.parse(line));
      } catch {
        parsed = undefined;
      }
      if (!parsed?.success)
        return emit({
          type: 'error',
          code: 'E_PROTOCOL',
          message: 'bad message',
          fatal: false,
        });
      const m = parsed.data;
      switch (m.type) {
        case 'ping':
          return emit({ type: 'pong', id: m.id });
        case 'permissions.check':
        case 'permissions.request':
          return emit(permissions());
        case 'shutdown':
          return die(0, null);
        case 'recording.start':
          audioPath = m.audioPath;
          // Mirrors the Swift helper: capture problems are non-fatal errors, recording.started still follows.
          if (
            opts.permissions?.['screen-recording'] === 'denied' &&
            opts.permissions?.microphone === 'denied'
          )
            emit({
              type: 'error',
              code: 'audio.unavailable',
              message: 'no audio source could start',
              fatal: false,
            });
          emit({ type: 'recording.started', audioPath });
          if (opts.scenario) startScript();
          return;
        case 'recording.pause':
          paused = true;
          return;
        case 'recording.resume':
          paused = false;
          return;
        case 'recording.stop':
          paused = false;
          clearScript();
          return emit({
            type: 'recording.stopped',
            audioPath,
            durationMs: opts.scenario ? FIXTURE_DURATION_MS : 0,
          });
      }
    },
    kill: () => die(null, 'SIGTERM'),
    crash: (code = 1) => die(code, null),
  };
}
