import {
  CAPTURE_SILENCE,
  HELPER_PROTOCOL_VERSION,
  type CaptureSource,
  HostToHelper,
  type HelperToHost,
  type RevocationDegradation,
} from './protocol';
import type { ChildHandle, Clock } from './supervisor';
import {
  arbiterStep,
  revocationDegradations,
  initialArbiterState,
  type HelperArbiterEvent,
  type HelperPermission,
} from './arbiter';
import { systemClock } from './supervisor';
import {
  FIXTURE_DURATION_MS,
  STANDUP_FIXTURE,
  fixtureActiveSpeakers,
  fixtureAsrEvents,
} from '../note/fixtures';

/** What the mock "hears" when you dictate (fillers + a stutter + a dictionary alias, so cleanup has work to do). */
export const MOCK_DICTATION =
  'um so I I think we should ship cue arts talk on friday';

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
  /** qrtz talk insertion outcome: AX works (default), only paste works, or nothing works. */
  insert?: 'ax' | 'paste' | 'fail';
  /** Dictation partial cadence (ms between scripted words). */
  dictationStepMs?: number;
  /**
   * H5 fault injection: revoke a TCC permission `afterMs` (real ms) after the first capture (recording.start or
   * dictation.start). Emits permission.revoked and applies the arbiter's effects like the Swift helper does.
   */
  revoke?: { permission: HelperPermission; afterMs: number }[];
  /**
   * Fault injection: the recording file cannot be created. Like the Swift helper, the mock fails closed: the
   * admitted recording is torn down at once (arbiter recordingFileFailed) and answered with
   * error{recording.file, audioPath}; no recording.started follows.
   */
  failRecordingFile?: boolean;
  /**
   * N2 fault injection: recording.stop is admitted but never acked (a wedged flush). `abort: 'ok'` answers
   * recording.abort{audioPath} with recording.aborted (the default); `abort: 'wedged'` never answers it either.
   */
  wedgeStop?: { abort?: 'ok' | 'wedged' };
  /**
   * N35 fault injection: per recording, the in-source mic fails `afterMs` (real ms) after recording.start and the
   * mock sends capture.degraded{degraded: ['no-mic'], reason}, like the Swift helper when the tap's or SCK's mic
   * engine dies. Like the Swift controller (312cc83): after recording.started (the timer fires after the start is
   * answered), once per recording, never after stop/abort (cleared with the script). The mic is no longer tracked:
   * scripted mic silence after it is not sent.
   */
  /**
   * N35 fault injection. `atStart` (N38): like Swift `DegradationInbox.goLive`, the start-time capture.degraded is
   * written right behind recording.started in the same turn (no timer), the ordering that hid N37.
   */
  micFails?: { afterMs: number; reason?: string; atStart?: boolean };
  /**
   * H8 fault injection: capture sources that fail to start on every recording.start. Each emits
   * capture.fallback{from, to, reason} before recording.started, like the Swift helper walking its source list.
   */
  captureFallback?: { from: string; to: string | null; reason: string }[];
  /**
   * N29 script: per recording, `source` reports `capture.silent` `afterMs` (real ms) after recording.start and, with
   * `audibleAfterMs`, `capture.audible` then. `sinceMs` defaults to when the silence would have begun
   * (elapsed − CAPTURE_SILENCE.afterMs, at least 0). Cleared on stop/abort like the scenario script.
   */
  silence?: {
    source: CaptureSource;
    afterMs: number;
    audibleAfterMs?: number;
    sinceMs?: number;
  }[];
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
  /** wedgeStop: the file whose stop never finished (until recording.abort). */
  let wedged: string | null = null;
  // H6/H5: same arbiter as the Swift helper (spec/vectors/helper/arbiter.json).
  let arb = initialArbiterState(
    (Object.entries(opts.permissions ?? {}) as [HelperPermission, string][])
      .filter(([, v]) => v === 'denied')
      .map(([k]) => k),
  );
  let revokeArmed = false;
  /** `start` names the ignored start, so a busy error can carry both its id/path and the active one. */
  const admit = (
    e: HelperArbiterEvent,
    start: { id?: string; audioPath?: string } = {},
  ) => {
    const r = arbiterStep(arb, e);
    arb = r.state;
    if (r.decision === 'ignore' && r.reason === 'already-recording')
      emit({
        type: 'error',
        code: 'recording.busy',
        message: 'a recording is already running',
        fatal: false,
        ...(start.audioPath ? { audioPath: start.audioPath } : {}),
        // The arbiter invariant guarantees a running recording has a file, so a busy always names it.
        ...(arb.activeAudioPath
          ? { activeAudioPath: arb.activeAudioPath }
          : {}),
      });
    else if (r.decision === 'ignore' && r.reason === 'already-dictating')
      emit({
        type: 'error',
        code: 'dictation.busy',
        message: 'a dictation is already running',
        fatal: false,
        ...(start.id ? { id: start.id } : {}),
        ...(dict ? { activeId: dict.id } : {}),
      });
    else if (r.decision === 'ignore')
      emit({
        type: 'log',
        level: 'warn',
        msg: `${'cmd' in e ? e.cmd : 'event'} while ${r.reason}; ignored`,
      });
    if (r.decision === 'refuse')
      emit({
        type: 'error',
        code: 'dictation.mic',
        message: 'microphone access denied',
        fatal: false,
      });
    if (r.decision === 'accept' && r.mux)
      emit({
        type: 'log',
        level: 'info',
        msg: 'mux: recording and dictation share the mic tap',
      });
    return r;
  };
  const armRevocations = () => {
    if (revokeArmed) return;
    revokeArmed = true;
    for (const rv of opts.revoke ?? [])
      at(rv.afterMs, () => {
        const r = arbiterStep(arb, { revoked: rv.permission });
        arb = r.state;
        if (r.decision !== 'accept') return;
        reported[rv.permission] = 'denied';
        emit({
          type: 'permission.revoked',
          permission: rv.permission,
          atMs: clock.now(),
          degraded: revocationDegradations(
            r.effects,
          ) as RevocationDegradation[],
        });
        if (r.effects.includes('dictation.stop:permission-revoked') && dict)
          stopDictation(dict.id, false, 'permission-revoked');
      });
  };
  let micFailed = false;
  const armSilence = () => {
    const t0 = clock.now();
    micFailed = false;
    if (opts.micFails && !opts.micFails.atStart)
      at(opts.micFails.afterMs, () => {
        micFailed = true;
        emit({
          type: 'capture.degraded',
          degraded: ['no-mic'],
          reason: opts.micFails?.reason ?? 'mock: mic device failed',
        });
      });
    for (const sl of opts.silence ?? []) {
      at(
        sl.afterMs,
        () =>
          (sl.source === 'mic' && micFailed) ||
          emit({
            type: 'capture.silent',
            source: sl.source,
            sinceMs:
              sl.sinceMs ??
              Math.max(0, clock.now() - t0 - CAPTURE_SILENCE.afterMs),
          }),
      );
      if (sl.audibleAfterMs !== undefined)
        at(
          sl.audibleAfterMs,
          () =>
            (sl.source === 'mic' && micFailed) ||
            emit({ type: 'capture.audible', source: sl.source }),
        );
    }
  };
  const reported: Partial<Record<HelperPermission, 'denied'>> = {};
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
    stopDictTimers();
    for (const cb of exitCbs) cb(code, signal);
  };
  const permissions = (): HelperToHost => ({
    type: 'permissions.status',
    status: {
      accessibility: granted,
      'screen-recording': granted,
      microphone: granted,
      ...opts.permissions,
      ...reported,
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

  // --- qrtz talk: scripted dictation (mic only) ---
  let dict: {
    id: string;
    timers: unknown[];
    words: number;
    t0: number;
  } | null = null;
  let dictCfg = { streamAudio: false, preRollMs: 0, warmMic: false };
  const DICTATION = MOCK_DICTATION.split(' ');
  const stopDictTimers = () => {
    for (const t of dict?.timers ?? []) clock.clearTimeout(t);
  };
  const startDictation = (id: string) => {
    const step = opts.dictationStepMs ?? 60;
    const d = { id, timers: [] as unknown[], words: 0, t0: clock.now() };
    dict = d;
    emit({
      type: 'dictation.started',
      id,
      preRollMs: dictCfg.warmMic ? dictCfg.preRollMs : 0,
    });
    for (let i = 1; i <= DICTATION.length; i++)
      d.timers.push(
        clock.setTimeout(() => {
          d.words = i;
          emit({
            type: 'dictation.asr',
            id,
            event: {
              kind: 'partial',
              utteranceId: `${id}-1`,
              startMs: 0,
              text: DICTATION.slice(0, i).join(' '),
            },
          });
          emit({
            type: 'dictation.level',
            rms: 0.3 + 0.5 * Math.abs(Math.sin(i)),
          });
          if (dictCfg.streamAudio)
            emit({
              type: 'dictation.audio',
              id,
              seq: i - 1,
              pcm16: 'AAAAAAAAAAA=',
            });
        }, i * step),
      );
  };
  const stopDictation = (
    id: string,
    cancel: boolean,
    reason?: 'permission-revoked',
  ) => {
    if (!dict || dict.id !== id) return;
    stopDictTimers();
    const text = cancel
      ? ''
      : DICTATION.slice(0, Math.max(dict.words, 1)).join(' ');
    if (!cancel)
      emit({
        type: 'dictation.asr',
        id,
        event: {
          kind: 'final',
          utteranceId: `${id}-1`,
          startMs: 0,
          endMs: clock.now() - dict.t0,
          text,
        },
      });
    emit({
      type: 'dictation.stopped',
      id,
      text,
      durationMs: cancel ? 0 : clock.now() - dict.t0,
      ...(reason ? { reason } : {}),
    });
    dict = null;
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
          if (
            admit(
              { cmd: 'recording.start', audioPath: m.audioPath },
              { audioPath: m.audioPath },
            ).decision !== 'accept'
          )
            return;
          if (opts.failRecordingFile) {
            // Fail closed: never leave a recording admitted without a file.
            admit({ recordingFileFailed: { audioPath: m.audioPath } });
            emit({
              type: 'error',
              code: 'recording.file',
              message: 'could not create the recording file',
              fatal: false,
              audioPath: m.audioPath,
            });
            return;
          }
          armRevocations();
          audioPath = m.audioPath;
          for (const f of opts.captureFallback ?? [])
            emit({ type: 'capture.fallback', ...f });
          armSilence();
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
          if (opts.micFails?.atStart) {
            micFailed = true;
            emit({
              type: 'capture.degraded',
              degraded: ['no-mic'],
              reason: opts.micFails.reason ?? 'mock: mic device failed',
            });
          }
          if (opts.scenario) startScript();
          return;
        case 'recording.abort':
          // Scoped: tears down only the recording writing this file (dictation untouched), without a flush.
          admit({ cmd: 'recording.abort', audioPath: m.audioPath });
          if (opts.wedgeStop?.abort === 'wedged') return;
          if (wedged === m.audioPath || audioPath === m.audioPath) {
            wedged = null;
            paused = false;
            clearScript();
          }
          emit({ type: 'recording.aborted', audioPath: m.audioPath });
          return;
        case 'recording.pause':
          paused = true;
          return;
        case 'recording.resume':
          paused = false;
          return;
        case 'dictation.configure':
          dictCfg = {
            streamAudio: m.streamAudio,
            preRollMs: m.preRollMs,
            warmMic: m.warmMic,
          };
          admit({ configure: { warmMic: m.warmMic } });
          return;
        case 'dictation.start':
          if (
            admit({ cmd: 'dictation.start' }, { id: m.id }).decision !==
            'accept'
          )
            return;
          armRevocations();
          return startDictation(m.id);
        case 'dictation.stop':
          if (dict?.id !== m.id) return;
          admit({ cmd: 'dictation.stop' });
          return stopDictation(m.id, false);
        case 'dictation.cancel':
          if (dict?.id !== m.id) return;
          admit({ cmd: 'dictation.cancel' });
          return stopDictation(m.id, true);
        case 'insert.text': {
          const mode = opts.insert ?? 'ax';
          return emit(
            mode === 'fail'
              ? {
                  type: 'insert.result',
                  id: m.id,
                  method: 'none',
                  ok: false,
                  app: 'Mock Editor',
                  error: 'no focused text field',
                }
              : {
                  type: 'insert.result',
                  id: m.id,
                  method: mode === 'ax' ? 'ax-selected-text' : 'paste',
                  ok: true,
                  app: 'Mock Editor',
                },
          );
        }
        case 'recording.stop':
          // A stop aimed at another file (stale-start cleanup racing a newer recording) is ignored.
          if (m.audioPath && arb.recording && m.audioPath !== audioPath) {
            emit({
              type: 'log',
              level: 'warn',
              msg: `recording.stop for ${m.audioPath} while recording ${audioPath}; ignored`,
            });
            return;
          }
          if (admit({ cmd: 'recording.stop' }).decision !== 'accept') return;
          if (opts.wedgeStop) {
            wedged = audioPath; // the flush never finishes: no recording.stopped
            return;
          }
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
