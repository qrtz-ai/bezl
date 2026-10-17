// In-page mocked IPC backend for Playwright against the dev UI (and `bun run dev:mock`). It sits behind
// the same `TauriBridge` the real app uses, routes through the real TS router with the real manifests
// (capability + zod checks), and plays a scripted meeting: start → streamed transcript (partials replaced by
// finals) → stop → finalize/summary/index → done. Query params inject faults:
//   ?mockFail=start-permission | summary-once   ?seed=1 (one finished meeting)   ?mockSpeed=<ms per tick>
import {
  AppError,
  TAURI_EVENT,
  TAURI_INVOKE_CMD,
  TAURI_SUBSCRIBE_CMD,
  applyConfigPatch,
  createRouter,
  defaultConfig,
  eventContract,
  isEventName,
  type Config,
  type EventName,
  type Handlers,
  type TauriBridge,
} from '@qrtz/core';
import {
  SAMPLE_MEETING_ID,
  idleSession,
  ipcSamples,
  unknownPermissions,
} from '@qrtz/core/ipc/samples';
import { resolveManifest } from '../shared/manifests';

type Json = Record<string, unknown>;
const params = new URLSearchParams(window.location.search);
const fail = params.get('mockFail') ?? '';
const tickMs = Number(params.get('mockSpeed') ?? '120');

const SCRIPT = [
  { speaker: 'Ada Lovelace', text: "Let's start with the roadmap." },
  { speaker: 'Grace Hopper', text: 'The beta ships on Friday.' },
  { speaker: 'Ada Lovelace', text: 'Great, I will write the release notes.' },
];

export function createMockBridge(): TauriBridge {
  const listeners = new Set<(e: { payload: unknown }) => void>();
  const subs = new Map<EventName, Set<string>>();
  const calls: { channel: string; subApp: string }[] = [];
  let config: Config = defaultConfig();
  const meetings: Json[] = [];
  const details = new Map<string, Json>();
  const pipelines = new Map<string, Json>();
  let session: Json = { ...idleSession };
  let live: Json | null = null;
  let timers: ReturnType<typeof setTimeout>[] = [];
  let summaryFailed = false;

  const emit = (event: EventName, payload: unknown) => {
    const parsed = eventContract[event].payload.parse(payload);
    const allowed = [...(subs.get(event) ?? [])];
    if (!allowed.length) return;
    for (const l of listeners)
      l({ payload: { event, payload: parsed, subApps: allowed } });
  };
  const setSession = (patch: Json) => {
    session = { ...session, ...patch };
    emit('note:session', session);
    return session;
  };
  const later = (ms: number, fn: () => void) => timers.push(setTimeout(fn, ms));

  if (params.get('seed') === '1') {
    const s = ipcSamples['note:get'].response as unknown as Json;
    meetings.push({ meeting: s.meeting });
    details.set(SAMPLE_MEETING_ID, s);
    pipelines.set(
      SAMPLE_MEETING_ID,
      ipcSamples['note:pipeline'].response as Json,
    );
  }

  const step = (status: string, attempts = 0, extra: Json = {}) => ({
    status,
    attempts,
    maxAttempts: 3,
    ...extra,
  });

  function runPipeline(id: string, from: 'finalize' | 'summary' = 'finalize') {
    const p: Json = {
      meetingId: id,
      stopReason: 'manual',
      steps: {
        finalize: step(from === 'finalize' ? 'running' : 'done', 1),
        summary: step(
          from === 'summary' ? 'running' : 'pending',
          from === 'summary' ? 1 : 0,
        ),
        index: step('pending'),
      },
      updatedAt: Date.now(),
    };
    const push = () => {
      p.updatedAt = Date.now();
      pipelines.set(id, structuredClone(p));
      emit('note:pipeline', p);
      if (session.meetingId === id)
        setSession({ pipeline: structuredClone(p) });
    };
    const steps = p.steps as Record<string, Json>;
    push();
    later(tickMs, () => {
      steps.finalize = step('done', 1);
      steps.summary = step('running', 1);
      push();
      later(tickMs, () => {
        if (fail === 'summary-once' && !summaryFailed) {
          summaryFailed = true;
          steps.summary = step('failed', 3, {
            error: {
              code: 'E_PROVIDER',
              message: 'LLM unavailable',
              retryable: true,
            },
          });
          push();
          setSession({
            phase: 'error',
            errorInfo: {
              code: 'E_PROVIDER',
              message: 'LLM unavailable',
              retryable: true,
            },
          });
          return;
        }
        steps.summary = step('done', 1);
        const d = details.get(id)!;
        d.summary = {
          summary: 'The beta ships on Friday.',
          actionItems: [
            { text: 'Write the release notes', owner: 'Ada Lovelace' },
          ],
          qa: [],
          provider: 'extractive',
        };
        steps.index = step('done', 1);
        push();
        const m = meetings.find((x) => (x.meeting as Json).id === id)!;
        (m.meeting as Json).status = 'done';
        (d.meeting as Json).status = 'done';
        setSession({ phase: 'done' });
        emit('note:changed', { id });
      });
    });
  }

  function startScript(id: string, startedAt: number) {
    let rev = 0;
    live = { meetingId: id, rev, segments: [], partial: null };
    SCRIPT.forEach((line, i) => {
      const words = line.text.split(' ');
      words.forEach((_, w) =>
        later(tickMs * (i * (words.length + 1) + w + 1), () => {
          const partial = {
            id: `u${i}`,
            text: words.slice(0, w + 1).join(' '),
            startMs: i * 3000,
          };
          const baseRev = rev++;
          (live as Json).partial = partial;
          (live as Json).rev = rev;
          emit('note:transcript', {
            meetingId: id,
            baseRev,
            rev,
            append: [],
            partial,
          });
          setSession({
            partial: partial.text,
            elapsedMs: Date.now() - startedAt,
            level: 0.3,
          });
        }),
      );
      later(tickMs * (i * (words.length + 1) + words.length + 1), () => {
        const seg = {
          id: `u${i}`,
          text: line.text,
          startMs: i * 3000,
          endMs: i * 3000 + 2500,
          speaker: line.speaker,
        };
        const baseRev = rev++;
        ((live as Json).segments as Json[]).push(seg);
        (live as Json).partial = null;
        (live as Json).rev = rev;
        emit('note:transcript', {
          meetingId: id,
          baseRev,
          rev,
          append: [seg],
          partial: null,
        });
        const lines = [
          ...((session.lines as Json[]) ?? []),
          {
            id: seg.id,
            speaker: seg.speaker,
            text: seg.text,
            startMs: seg.startMs,
          },
        ];
        setSession({ lines, partial: '', activeSpeaker: line.speaker });
      });
    });
  }

  const handlers: Partial<Handlers> = {
    'app:get-info': () => ({
      ...ipcSamples['app:get-info'].response,
      electron: 'tauri (mock ipc)',
    }),
    'config:get': () => config,
    'config:set': ({ patch }) => {
      config = applyConfigPatch(config, patch);
      emit('config:changed', config);
      return config;
    },
    'note:set-auto-record': ({ enabled }) => {
      config = applyConfigPatch(config, { note: { autoRecord: enabled } });
      emit('config:changed', config);
      return { autoRecord: enabled };
    },
    'secrets:list': () => [],
    'secrets:set': () => ({ ok: true as const }),
    'db:health': () => ipcSamples['db:health'].response,
    'helper:status': () => ipcSamples['helper:status'].response,
    'permissions:get': () => unknownPermissions,
    'permissions:request': ({ permission }) => ({
      ...unknownPermissions,
      [permission]: 'granted',
    }),
    'note:session': () => session as never,
    'note:dismiss': () => {
      if (['done', 'error'].includes(String(session.phase)))
        session = { ...idleSession };
      return setSession({}) as never;
    },
    'note:start': (req) => {
      if (fail === 'start-permission') {
        // Same as NoteService: surface the refusal as state (every window renders it), then reject.
        const message =
          'Recording needs Screen & System Audio Recording or Microphone access. Grant one in System Settings › Privacy & Security, then try again.';
        setSession({
          error: message,
          errorInfo: { code: 'permission-denied', message, retryable: true },
        });
        throw new AppError('E_FORBIDDEN', message);
      }
      if (
        ['starting', 'recording', 'reconnecting', 'stopping'].includes(
          String(session.phase),
        )
      )
        throw new AppError('E_VALIDATION', 'already recording');
      timers.forEach(clearTimeout);
      timers = [];
      const id = crypto.randomUUID();
      const startedAt = Date.now();
      const meeting = {
        id,
        title: req?.title ?? 'untitled meeting',
        platform: 'other',
        startedAt,
        participants: ['Ada Lovelace', 'Grace Hopper'],
        status: 'recording',
      };
      meetings.unshift({ meeting });
      details.set(id, { meeting, segments: [], notes: [] });
      setSession({
        ...idleSession,
        phase: 'starting',
        meetingId: id,
        startedAt,
      });
      later(tickMs, () =>
        setSession({ phase: 'recording', participants: meeting.participants }),
      );
      startScript(id, startedAt);
      emit('note:changed', { id });
      return session as never;
    },
    'note:stop': () => {
      if (
        !['starting', 'recording', 'reconnecting'].includes(
          String(session.phase),
        )
      )
        throw new AppError('E_VALIDATION', 'not recording');
      timers.forEach(clearTimeout);
      timers = [];
      const id = String(session.meetingId);
      const d = details.get(id)!;
      const segs = ((live?.segments as Json[]) ?? []).map((s) => ({
        id: String(s.id),
        startMs: s.startMs,
        endMs: s.endMs,
        text: s.text,
        speakerName: s.speaker,
        source: 'live',
      }));
      d.segments = segs;
      const endedAt = Date.now();
      Object.assign(d.meeting as Json, {
        status: 'processing',
        endedAt,
        durationMs: endedAt - Number(session.startedAt),
      });
      setSession({ phase: 'processing', stopReason: 'manual', partial: '' });
      runPipeline(id);
      emit('note:changed', { id });
      return session as never;
    },
    'note:pause': (req) =>
      setSession({ paused: req.paused ?? !session.paused }) as never,
    'note:pipeline': ({ id }) => (pipelines.get(id) ?? null) as never,
    'note:retry-step': ({ id }) => {
      setSession({ phase: 'processing', errorInfo: undefined });
      runPipeline(id, 'summary');
      return pipelines.get(id) as never;
    },
    'note:live-transcript': () => live as never,
    'note:list': ({ query }) =>
      meetings.filter(
        (m) =>
          !query ||
          String((m.meeting as Json).title)
            .toLowerCase()
            .includes(query.toLowerCase()),
      ) as never,
    'note:get': ({ id }) => {
      const d = details.get(id);
      if (!d) throw new AppError('E_NOT_FOUND', `meeting ${id} not found`);
      return d as never;
    },
    'note:list-notes': ({ meetingId }) =>
      (details.get(meetingId ?? String(session.meetingId))?.notes as never) ??
      [],
    'note:add-note': ({ meetingId, text, atMs }) => {
      const id = meetingId ?? String(session.meetingId);
      const n = {
        id: crypto.randomUUID(),
        atMs: atMs ?? 0,
        text,
        createdAt: Date.now(),
      };
      ((details.get(id)?.notes as Json[]) ?? []).push(n);
      return { ...n, meetingId: id };
    },
    'note:delete': ({ id }) => {
      const i = meetings.findIndex((m) => (m.meeting as Json).id === id);
      if (i >= 0) meetings.splice(i, 1);
      emit('note:changed', { id });
      return { ok: true as const };
    },
    'note:undo-delete': () => ({ restored: false }),
    'note:export': ({ id }) => ({
      filename: `${id}.md`,
      markdown: '# meeting\n',
    }),
    'hud:set-expanded': ({ expanded }) => ({ expanded }),
    'talk:state': () => ipcSamples['talk:state'].response,
    'talk:history': () => [],
    'talk:dictionary': () => [],
  };

  const router = createRouter({ handlers, resolveSubApp: resolveManifest });
  const mock = {
    calls,
    emit,
    reset: () => {
      timers.forEach(clearTimeout);
      session = { ...idleSession };
    },
  };
  (window as unknown as { __qrtzMock: typeof mock }).__qrtzMock = mock;

  return {
    async invoke(cmd, args) {
      if (cmd === TAURI_INVOKE_CMD) {
        const env = (args?.envelope ?? {}) as {
          channel?: string;
          subApp?: string;
        };
        calls.push({
          channel: String(env.channel),
          subApp: String(env.subApp),
        });
        return router.dispatch(args?.envelope);
      }
      if (cmd === TAURI_SUBSCRIBE_CMD) {
        const subApp = String(args?.subApp);
        const event = String(args?.event);
        const m = resolveManifest(subApp);
        if (
          !isEventName(event) ||
          !m?.capabilities.includes(eventContract[event].capability)
        )
          return {
            ok: false,
            error: new AppError(
              'E_FORBIDDEN',
              `"${subApp}" may not subscribe to "${event}"`,
            ).toJSON(),
          };
        let set = subs.get(event);
        if (!set) subs.set(event, (set = new Set()));
        set.add(subApp);
        return { ok: true };
      }
      throw new Error(`unknown command ${cmd}`);
    },
    async listen(event, cb) {
      if (event !== TAURI_EVENT) throw new Error(`unexpected event ${event}`);
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  };
}
