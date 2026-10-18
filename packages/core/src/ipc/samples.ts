import type { IpcChannel, IpcRequest, IpcResponse } from './contract';
import type { EventName, EventPayload } from './events';

/**
 * Golden IPC samples: one request/response per channel and one payload per event, in the shape the
 * backend actually sends (zod *output*). Three consumers keep both runtimes honest:
 *  - vitest parses every sample with the zod contract (contract-json.test.ts),
 *  - it renders them to contracts/ipc-samples.json, which `cargo test` validates against the generated
 *    JSON Schema and uses as the Rust stub ports' idle answers,
 *  - the Tauri Playwright suite's mocked IPC backend serves them.
 * `idle` marks the answer a backend with no data gives (empty DB, nothing recording).
 */
export interface ChannelSample<C extends IpcChannel> {
  request: IpcRequest<C>;
  response: IpcResponse<C>;
}

const T0 = 1_760_659_200_000;
const speechNone = {
  configured: true,
  installed: null,
  bytes: 0,
  latest: null,
  latestBytes: null,
  waiting: null,
  listExpired: false,
  download: { state: 'idle' as const, done: 0, total: 0 },
};
const speechInstalled = {
  ...speechNone,
  installed: '1.0.0',
  bytes: 412_000_000,
  latest: '1.0.0',
  latestBytes: 412_000_000,
};
const speechDownloading = {
  ...speechNone,
  latest: '1.0.0',
  latestBytes: 412_000_000,
  download: {
    state: 'downloading' as const,
    done: 100_000_000,
    total: 412_000_000,
  },
};
export const SAMPLE_MEETING_ID = '00000000-0000-4000-8000-000000000001';
const meeting = {
  id: SAMPLE_MEETING_ID,
  title: 'Weekly Product Sync',
  platform: 'google-meet' as const,
  startedAt: T0,
  endedAt: T0 + 1_800_000,
  participants: ['Ada Lovelace', 'Grace Hopper'],
  status: 'done' as const,
  durationMs: 1_800_000,
};
const summary = {
  summary: 'Agreed to ship the beta on Friday.',
  actionItems: [{ text: 'Ship the beta', owner: 'Ada Lovelace' }],
  qa: [{ question: 'Is QA done?', answer: 'Yes.', askedBy: 'Grace Hopper' }],
  provider: 'extractive',
};
const step = (status: 'done' | 'pending') => ({
  status,
  attempts: status === 'done' ? 1 : 0,
  maxAttempts: 3,
});
const pipeline = {
  meetingId: SAMPLE_MEETING_ID,
  stopReason: 'manual' as const,
  steps: { finalize: step('done'), summary: step('done'), index: step('done') },
  updatedAt: T0 + 1_801_000,
};
export const idleSession = {
  phase: 'idle' as const,
  elapsedMs: 0,
  participants: [],
  level: 0,
  lines: [],
  partial: '',
  helper: 'ok' as const,
  paused: false,
  degraded: [],
};
const recordingSession = {
  ...idleSession,
  phase: 'recording' as const,
  meetingId: SAMPLE_MEETING_ID,
  startedAt: T0,
};
export const unknownPermissions = {
  accessibility: 'unknown',
  'screen-recording': 'unknown',
  microphone: 'unknown',
} as const;
const note = {
  id: 'n1',
  atMs: 1000,
  text: 'Follow up on pricing',
  createdAt: T0,
};
const talkIdle = { phase: 'idle' as const, text: '', partial: '', level: 0 };

const ctrlTask = {
  id: 'task-0',
  text: 'every week clean up my downloads folder',
  schedule: { kind: 'weekly' as const, weekday: 0, hour: 18, minute: 0 },
  paused: false,
  createdAt: T0,
  runs: 1,
  lastRunAt: T0 + 60_000,
  nextRunAt: T0 + 7 * 86_400_000,
};
const ctrlRun = {
  id: 'run-1',
  taskId: 'task-0',
  at: T0 + 60_000,
  status: 'pending-intent' as const,
  events: 2,
  violations: 0,
};
const ctrlSamples = {
  'ctrl:tasks': { request: undefined, response: [ctrlTask] },
  'ctrl:create-task': {
    request: { text: ctrlTask.text, schedule: ctrlTask.schedule },
    response: {
      ...ctrlTask,
      runs: 0,
      lastRunAt: null,
      nextRunAt: T0 + 7 * 86_400_000,
    },
  },
  'ctrl:set-paused': {
    request: { id: 'task-0', paused: true },
    response: { ...ctrlTask, paused: true, nextRunAt: null },
  },
  'ctrl:delete-task': { request: { id: 'task-0' }, response: { ok: true } },
  'ctrl:run-now': { request: { id: 'task-0' }, response: ctrlRun },
  'ctrl:runs': { request: { taskId: 'task-0' }, response: [ctrlRun] },
  'ctrl:run': {
    request: { runId: 'run-1', at: 2 },
    response: {
      run: ctrlRun,
      events: [
        { seq: 0, kind: 'task.started', at: T0 + 60_000, actionId: null },
        { seq: 1, kind: 'intent.pending', at: T0 + 60_000, actionId: null },
      ],
      at: 2,
      state: {
        actions: [],
        violations: 0,
        inconclusive: 0,
        quarantined: [],
        status: 'pending-intent',
      },
    },
  },
} satisfies { [C in Extract<IpcChannel, `ctrl:${string}`>]: ChannelSample<C> };

export const ipcSamples: { [C in IpcChannel]: ChannelSample<C> } = {
  'app:get-info': {
    request: undefined,
    response: {
      name: 'qrtz',
      version: '0.1.0',
      platform: 'darwin',
      arch: 'arm64',
      electron: 'tauri 2',
    },
  },
  'config:get': { request: undefined, response: {} },
  'config:set': {
    request: { patch: { theme: 'light' } },
    response: { theme: 'light' },
  },
  'db:health': {
    request: undefined,
    response: { ok: true, schemaVersion: 5, fts5: true },
  },
  'helper:status': {
    request: undefined,
    response: { state: 'running', restarts: 0 },
  },
  'permissions:get': { request: undefined, response: unknownPermissions },
  'permissions:request': {
    request: { permission: 'microphone' },
    response: { ...unknownPermissions, microphone: 'granted' },
  },
  'note:session': { request: undefined, response: idleSession },
  'note:start': { request: { title: 'Standup' }, response: recordingSession },
  'note:stop': {
    request: undefined,
    response: {
      ...recordingSession,
      phase: 'processing',
      stopReason: 'manual',
    },
  },
  'note:pipeline': { request: { id: SAMPLE_MEETING_ID }, response: pipeline },
  'note:retry-step': {
    request: { id: SAMPLE_MEETING_ID, step: 'summary' },
    response: pipeline,
  },
  'note:live-transcript': {
    request: { meetingId: SAMPLE_MEETING_ID },
    response: {
      meetingId: SAMPLE_MEETING_ID,
      rev: 2,
      segments: [
        {
          id: 'u1',
          text: "Let's start.",
          startMs: 0,
          endMs: 1200,
          speaker: 'Ada Lovelace',
        },
      ],
      partial: { id: 'u2', text: 'sounds', startMs: 1300 },
    },
  },
  'note:dismiss': { request: undefined, response: idleSession },
  'note:list': { request: { query: '' }, response: [{ meeting }] },
  'note:get': {
    request: { id: SAMPLE_MEETING_ID },
    response: {
      meeting,
      segments: [
        {
          id: 's1',
          startMs: 0,
          endMs: 1200,
          text: "Let's start.",
          speakerName: 'Ada Lovelace',
          source: 'live',
        },
      ],
      summary,
      notes: [note],
    },
  },
  'note:export': {
    request: { id: SAMPLE_MEETING_ID, save: false },
    response: {
      filename: 'weekly-product-sync.md',
      markdown: '# Weekly Product Sync\n',
    },
  },
  'note:resummarize': { request: { id: SAMPLE_MEETING_ID }, response: summary },
  'note:set-auto-record': {
    request: { enabled: true },
    response: { autoRecord: true },
  },
  'note:rename-speaker': {
    request: { meetingId: SAMPLE_MEETING_ID, from: 'Speaker 1', to: 'Ada' },
    response: { changed: 3 },
  },
  'note:delete': {
    request: { id: SAMPLE_MEETING_ID, deferMs: 5000 },
    response: { ok: true },
  },
  'note:undo-delete': {
    request: { id: SAMPLE_MEETING_ID },
    response: { restored: true },
  },
  'note:add-note': {
    request: { text: 'Follow up on pricing' },
    response: { ...note, meetingId: SAMPLE_MEETING_ID },
  },
  'note:update-note': {
    request: { id: 'n1', text: 'Edited' },
    response: { ok: true },
  },
  'note:delete-note': { request: { id: 'n1' }, response: { ok: true } },
  'note:list-notes': {
    request: { meetingId: SAMPLE_MEETING_ID },
    response: [note],
  },
  'note:set-template': {
    request: { id: SAMPLE_MEETING_ID, template: 'standup' },
    response: { ...summary, template: 'standup' },
  },
  'hud:set-expanded': {
    request: { expanded: true },
    response: { expanded: true },
  },
  'note:pause': {
    request: { paused: true },
    response: { ...recordingSession, paused: true },
  },
  'data:export-backup': {
    request: {},
    response: {
      savedTo: '/tmp/qrtz-backup.zip',
      meetings: 1,
      audioFiles: 1,
      bytes: 1024,
    },
  },
  'data:import-backup': {
    request: {},
    response: { imported: 1, skipped: 0, audioFiles: 1 },
  },
  'talk:state': { request: undefined, response: talkIdle },
  'talk:start': {
    request: undefined,
    response: {
      ...talkIdle,
      phase: 'listening',
      id: 't1',
      startedAt: T0,
      engine: 'on-device',
    },
  },
  'talk:stop': {
    request: undefined,
    response: { ...talkIdle, phase: 'finalizing', id: 't1' },
  },
  'talk:cancel': { request: undefined, response: talkIdle },
  'talk:history': { request: undefined, response: [] },
  'talk:delete-entry': { request: { id: 't1' }, response: { ok: true } },
  'talk:clear-history': { request: undefined, response: { ok: true } },
  'talk:copy': { request: { id: 't1' }, response: { ok: true } },
  'talk:dictionary': { request: undefined, response: [] },
  'talk:set-dictionary': {
    request: { entries: [{ term: 'qrtz', aliases: ['quartz'] }] },
    response: [{ term: 'qrtz', aliases: ['quartz'] }],
  },
  'log:write': {
    request: { level: 'error', scope: 'player', message: 'media error 4' },
    response: { accepted: true },
  },
  'logs:export': {
    request: {},
    response: {
      savedTo: '/Users/me/Documents/qrtz-logs-0.2.0.log',
      files: 2,
      bytes: 48_213,
    },
  },
  'speech:status': { request: undefined, response: speechInstalled },
  'speech:check': { request: undefined, response: speechInstalled },
  'speech:download': { request: {}, response: speechDownloading },
  'speech:pause': {
    request: undefined,
    response: {
      ...speechDownloading,
      download: { ...speechDownloading.download, state: 'paused' },
    },
  },
  'speech:resume': { request: undefined, response: speechDownloading },
  'speech:delete': { request: undefined, response: speechNone },
  ...ctrlSamples,
};

export const eventSamples: { [E in EventName]: EventPayload<E> } = {
  'helper:status': { state: 'running', restarts: 0 },
  'config:changed': { theme: 'dark' },
  'note:session': recordingSession,
  'note:pipeline': pipeline,
  'note:transcript': {
    meetingId: SAMPLE_MEETING_ID,
    baseRev: 1,
    rev: 2,
    append: [{ id: 'u1', text: "Let's start.", startMs: 0, endMs: 1200 }],
    partial: null,
  },
  'note:changed': { id: SAMPLE_MEETING_ID },
  'data:backup-progress': { kind: 'export', phase: 'write', done: 1, total: 2 },
  'talk:state': talkIdle,
  'talk:history-changed': { id: 't1' },
  'permissions:changed': unknownPermissions,
  'speech:status': speechDownloading,
  'ctrl:changed': { ids: ['task-0'] },
};

/** Idle answers per channel (what an empty backend says); a channel absent here has no idle answer. */
export const idleResponses: Partial<Record<IpcChannel, unknown>> = {
  'note:session': idleSession,
  'note:dismiss': idleSession,
  'note:list': [],
  'note:list-notes': [],
  'note:pipeline': null,
  'note:live-transcript': null,
  'permissions:get': unknownPermissions,
  'talk:state': talkIdle,
  'talk:cancel': talkIdle,
  'talk:history': [],
  'talk:dictionary': [],
  'speech:status': { ...speechNone, configured: false },
  'ctrl:tasks': [],
};
