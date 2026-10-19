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
/** ADR 0181: qrtz's own update row before any check, and with a newer release found. */
export const appUpdateIdle = {
  current: '0.2.0',
  state: 'idle',
  latest: null,
  notes: null,
  sizeBytes: null,
  doneBytes: null,
  error: null,
  checkedAtMs: null,
} as const;
export const appUpdateAvailable = {
  ...appUpdateIdle,
  state: 'available',
  latest: '0.2.1',
  notes: 'qrtz updates itself from settings.',
  sizeBytes: 31_400_000,
  checkedAtMs: 1_792_400_000_000,
} as const;

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

const ctrlAsk = {
  decisionId: 'd2',
  node: 'choose-move' as const,
  question: 'what should ctrl do next?',
  options: [
    {
      id: 'menu.m:60:1',
      label: 'switch workspace… (menu)',
      p: 0.31,
      gated: false,
    },
    { id: 'branch.e:60:2', label: 'look in web area', p: 0.22, gated: false },
    { id: 'key.cmd+f', label: 'find (cmd+f)', p: 0.12, gated: false },
  ],
};
const ctrlView = {
  runId: 'run-1',
  doId: 'do-0',
  text: 'open youtube',
  status: 'done' as const,
  stop: null,
  app: 'safari',
  startedAt: T0 + 60_000,
  at: 3,
  total: 3,
  events: [
    { seq: 0, kind: 'do.started' },
    { seq: 1, kind: 'obs.apps' },
    { seq: 2, kind: 'decision.made' },
  ],
  decisions: [
    {
      id: 'd0',
      node: 'choose-app' as const,
      question: 'which app should do it?',
      chosen: 'app.com.apple.Safari',
      chosenLabel: 'open safari',
      p: 0.93,
      outcome: 'act' as const,
      answered: false,
      backtracks: 0,
      model: 'qrtz-heuristic 2',
      options: 7,
    },
  ],
  actions: [
    {
      id: 'a0',
      kind: 'open_app' as const,
      label: 'open safari',
      status: 'done' as const,
      err: null,
    },
    {
      id: 'a1',
      kind: 'key_combo' as const,
      label: 'press cmd+l',
      status: 'done' as const,
      err: null,
    },
    {
      id: 'a2',
      kind: 'set_value' as const,
      label: 'type “youtube.com” and press return',
      status: 'done' as const,
      err: null,
    },
  ],
  ask: null,
  confirm: null,
  backtracks: 0,
};
const ctrlSamples = {
  'ctrl:do': { request: { text: 'open youtube' }, response: ctrlView },
  'ctrl:runs': {
    request: undefined,
    response: [
      {
        runId: 'run-1',
        doId: 'do-0',
        text: 'open youtube',
        status: 'done',
        startedAt: T0 + 60_000,
        actions: 3,
      },
    ],
  },
  'ctrl:run': {
    request: { runId: 'run-3', at: 2 },
    response: {
      ...ctrlView,
      runId: 'run-3',
      doId: 'do-2',
      text: 'message the design channel in slack',
      status: 'asking',
      app: 'slack',
      at: 2,
      actions: [{ ...ctrlView.actions[0]!, label: 'open slack' }],
      ask: ctrlAsk,
    },
  },
  'ctrl:answer': {
    request: { runId: 'run-3', decisionId: 'd2', optionId: null },
    response: { ...ctrlView, runId: 'run-3', status: 'cancelled', ask: null },
  },
  'ctrl:confirm': {
    request: { runId: 'run-5', gateId: 'g0', answer: 'approve' },
    response: {
      ...ctrlView,
      runId: 'run-5',
      text: 'press send in mail',
      app: 'mail',
    },
  },
  'ctrl:stop': {
    request: { runId: 'run-5' },
    response: { runId: 'run-5', status: 'stopping' },
  },
  'ctrl:model': {
    request: undefined,
    response: {
      consented: false,
      state: 'off',
      sizeBytes: 0,
      bytesOnDisk: 0,
      scorer: 'heuristic',
      error: null,
    },
  },
  'ctrl:model-consent': {
    request: { consent: true },
    response: {
      consented: true,
      state: 'unavailable',
      sizeBytes: 0,
      bytesOnDisk: 0,
      scorer: 'heuristic',
      error: 'laya.placeholder',
    },
  },
} satisfies { [C in Extract<IpcChannel, `ctrl:${string}`>]: ChannelSample<C> };

const snapSamples = {
  'snap:status': {
    request: undefined,
    response: {
      running: true,
      axTrusted: true,
      registered: { leftHalf: 'alt+cmd+left', rightHalf: 'alt+cmd+right' },
      refused: { undo: 'another app uses these keys' },
    },
  },
  'snap:request-access': {
    request: undefined,
    response: { opened: true, axTrusted: false },
  },
} satisfies { [C in Extract<IpcChannel, `snap:${string}`>]: ChannelSample<C> };

/** ADR 0228: Agent Dash. */
const dashSamples = {
  'dash:view': {
    request: undefined,
    response: {
      origin: 'https://dash.qrtz.ai',
      readmeVersion: '2.1',
      deleted: false,
      retentionDays: 30,
      agents: [],
      audit: [],
    },
  },
  'dash:issue-code': {
    request: undefined,
    response: { code: 'ENR-7F3A-91C2', expiresAt: T0 + 900_000 },
  },
  'dash:rotate-codes': { request: undefined, response: { ok: true as const } },
  'dash:review-agent': {
    request: { agentId: 'ag_2', approve: true },
    response: { ok: true },
  },
  'dash:delete': {
    request: { scope: 'widget' as const, agentId: 'ag_2', key: 'state-farm' },
    response: { ok: true },
  },
} satisfies { [C in Extract<IpcChannel, `dash:${string}`>]: ChannelSample<C> };

/** ADR 0217: nano's Apple Calendar. */
const calendarStatus = {
  enabled: true,
  access: 'granted' as const,
  calendars: [
    { id: 'cal-work', title: 'Work' },
    { id: 'cal-home', title: 'Home' },
  ],
  chosen: null,
  lastSyncAt: 1_790_000_000_000,
};
const calendarSamples = {
  'calendar:status': { request: undefined, response: calendarStatus },
  'calendar:connect': {
    request: undefined,
    response: { access: 'granted' as const },
  },
  'calendar:access': {
    request: undefined,
    response: { access: 'granted' as const },
  },
  'calendar:refresh': { request: undefined, response: calendarStatus },
  'calendar:upcoming': {
    request: undefined,
    response: {
      events: [
        {
          id: 'ev-1@1790000000000',
          calendarId: 'cal-work',
          title: 'weekly sync',
          startAt: 1_790_000_000_000,
          endAt: 1_790_001_800_000,
          allDay: false,
          attendeeCount: 4,
          meetingUrl: 'https://meet.google.com/abc-defg-hij',
        },
      ],
    },
  },
} satisfies {
  [C in Extract<IpcChannel, `calendar:${string}`>]: ChannelSample<C>;
};

/** ADR 0220: the notes vault. */
const vaultInfo = {
  path: '/Users/me/Documents/qrtz Notes',
  isDefault: true,
  count: 1,
};
const doc = {
  id: '0192a1b2c3d4e5f60718293a',
  title: 'interview prep',
  path: 'interview prep.md',
  updatedAt: 1_790_000_000_000,
  meetings: [],
  body: '# interview prep\n\nask about the team\n',
  hash: '00112233445566778899aabbccddeeff',
  readOnly: false,
};
const { body: _b, hash: _h, readOnly: _r, ...docSummary } = doc;
const docSamples = {
  'vault:get': { request: undefined, response: vaultInfo },
  'vault:choose': {
    request: undefined,
    response: { ...vaultInfo, canceled: false },
  },
  'doc:list': { request: { query: 'team' }, response: [docSummary] },
  'doc:get': { request: { id: doc.id }, response: doc },
  'doc:create': { request: { title: 'interview prep' }, response: doc },
  'doc:save': {
    request: { id: doc.id, body: doc.body, baseHash: doc.hash },
    response: doc,
  },
  'doc:trash': { request: { id: doc.id }, response: { ok: true } },
  'doc:link-meeting': {
    request: { id: doc.id, meetingId: SAMPLE_MEETING_ID },
    response: { ...doc, meetings: [SAMPLE_MEETING_ID] },
  },
  'doc:unlink-meeting': {
    request: { id: doc.id, meetingId: SAMPLE_MEETING_ID },
    response: doc,
  },
  'doc:rebuild-index': { request: undefined, response: { count: 1 } },
} satisfies {
  [
    C in Extract<IpcChannel, `doc:${string}` | `vault:${string}`>
  ]: ChannelSample<C>;
};

const packagesState = {
  packages: [
    {
      name: 'echo',
      version: '0.3.0',
      kind: 'extension' as const,
      requested: '^0.3.0',
      requiredBy: [],
      usedBy: [],
      disabled: null,
      sizeBytes: 9_400_000,
    },
    {
      name: 'inference.voice',
      version: '1.0.0',
      kind: 'service' as const,
      requested: null,
      requiredBy: ['echo@0.3.0'],
      usedBy: ['echo'],
      disabled: null,
      sizeBytes: 4_100_000,
    },
    {
      name: 'model.parakeet-tdt-v2-en',
      version: '1.0.0',
      kind: 'model' as const,
      requested: null,
      requiredBy: ['echo@0.3.0'],
      usedBy: ['echo'],
      disabled: null,
      sizeBytes: 615_000_000,
    },
  ],
  index: { version: 12, issuedMs: 1_790_000_000_000 },
  available: [
    {
      name: 'echo',
      kind: 'extension' as const,
      version: '0.3.0',
      versions: ['0.3.0', '0.2.0'],
      sizeBytes: 9_400_000,
      installSizeBytes: 628_500_000,
      dependencies: [
        {
          name: 'inference.voice',
          range: '^1.0.0',
          version: '1.0.0',
          kind: 'service' as const,
          sizeBytes: 4_100_000,
          direct: true,
        },
        {
          name: 'model.parakeet-tdt-v2-en',
          range: '^1.0.0',
          version: '1.0.0',
          kind: 'model' as const,
          sizeBytes: 615_000_000,
          direct: false,
        },
      ],
      permissions: ['microphone' as const, 'accessibility' as const],
      minOs: '14.2',
      teamId: 'ABCDE12345',
    },
    {
      name: 'nano',
      kind: 'extension' as const,
      version: '0.3.0',
      versions: ['0.3.0'],
      sizeBytes: 12_200_000,
      installSizeBytes: 631_300_000,
      dependencies: [
        {
          name: 'inference.voice',
          range: '^1.0.0',
          version: '1.0.0',
          kind: 'service' as const,
          sizeBytes: 4_100_000,
          direct: true,
        },
        {
          name: 'model.parakeet-tdt-v2-en',
          range: '^1.0.0',
          version: '1.0.0',
          kind: 'model' as const,
          sizeBytes: 615_000_000,
          direct: false,
        },
      ],
      permissions: [
        'microphone' as const,
        'screen-recording' as const,
        'accessibility' as const,
      ],
      minOs: '14.2',
      teamId: 'ABCDE12345',
    },
    {
      name: 'ctrl',
      kind: 'extension' as const,
      version: '0.1.0',
      versions: ['0.1.0'],
      sizeBytes: 6_800_000,
      installSizeBytes: 490_800_000,
      dependencies: [
        {
          name: 'model.laya',
          range: '^1.0.0',
          version: '1.0.0',
          kind: 'model' as const,
          sizeBytes: 484_000_000,
          direct: true,
        },
      ],
      permissions: ['accessibility' as const],
      minOs: '14.2',
      teamId: 'ABCDE12345',
    },
  ],
  reclaimable: { dirs: 0, blobs: 0, bytes: 0 },
  pendingRestart: [],
};
const packagePlan = {
  planSha256: 'ab'.repeat(32),
  changes: [
    { change: 'add' as const, name: 'echo', version: '0.3.0' },
    { change: 'add' as const, name: 'inference.voice', version: '1.0.0' },
    {
      change: 'add' as const,
      name: 'model.parakeet-tdt-v2-en',
      version: '1.0.0',
    },
  ],
  downloadBytes: 628_500_000,
};

export const ipcSamples: { [C in IpcChannel]: ChannelSample<C> } = {
  'app:licenses': {
    request: undefined,
    response: { text: '# Third-party notices\n' },
  },
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
  'permissions:open-settings': {
    request: { pane: 'accessibility' },
    response: { opened: true, restart: false },
  },
  'app:restart': { request: undefined, response: { restarting: true } },
  'onboarding:finish': { request: undefined, response: { done: true } },
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
  'note:keep-recording': { request: undefined, response: recordingSession },
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
  'module:get-settings': {
    request: undefined,
    response: { talk: { showPill: true } },
  },
  'module:set-settings': {
    request: { patch: { talk: { showPill: false } } },
    response: { talk: { showPill: false } },
  },
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
  'packages:list': { request: undefined, response: packagesState },
  'packages:refresh': { request: undefined, response: packagesState },
  'packages:check-update': {
    request: undefined,
    response: {
      current: '0.3.0',
      state: 'not-set-up',
      latest: null,
      sizeBytes: null,
    },
  },
  'app-update:status': { request: undefined, response: appUpdateIdle },
  'app-update:check': { request: undefined, response: appUpdateAvailable },
  'app-update:download': {
    request: undefined,
    response: { ...appUpdateAvailable, state: 'downloading', doneBytes: 0 },
  },
  'app-update:restart': { request: undefined, response: { restarting: true } },
  'packages:plan': {
    request: { op: 'install', names: ['echo'] },
    response: packagePlan,
  },
  'packages:apply': {
    request: {
      planSha256: packagePlan.planSha256,
      downloadBytes: packagePlan.downloadBytes,
    },
    response: { changes: packagePlan.changes, staleRevocation: false },
  },
  'packages:prune': {
    request: undefined,
    response: { dirs: 1, blobs: 4, bytes: 615_000_000 },
  },
  'packages:set-enabled': {
    request: { name: 'ctrl', enabled: false },
    response: { name: 'ctrl', enabled: false },
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
  ...snapSamples,
  ...dashSamples,
  ...calendarSamples,
  ...docSamples,
};

export const eventSamples: { [E in EventName]: EventPayload<E> } = {
  'helper:status': { state: 'running', restarts: 0 },
  'config:changed': { theme: 'dark' },
  'snap:changed': snapSamples['snap:status'].response,
  'calendar:changed': calendarStatus,
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
  'note:auto-stop': { state: 'armed', deadlineAt: 1760000180000 },
  'doc:changed': { ids: [doc.id] },
  'data:backup-progress': { kind: 'export', phase: 'write', done: 1, total: 2 },
  'talk:state': talkIdle,
  'talk:history-changed': { id: 't1' },
  'permissions:changed': unknownPermissions,
  'speech:status': speechDownloading,
  'ctrl:changed': { ids: ['task-0'] },
  'packages:progress': { doneBytes: 314_000_000, totalBytes: 628_500_000 },
  'app-update:state': appUpdateAvailable,
};

/** Idle answers per channel (what an empty backend says); a channel absent here has no idle answer. */
export const idleResponses: Partial<Record<IpcChannel, unknown>> = {
  'app-update:status': appUpdateIdle,
  'note:session': idleSession,
  'note:dismiss': idleSession,
  'note:list': [],
  'note:list-notes': [],
  'doc:list': [],
  'note:pipeline': null,
  'note:live-transcript': null,
  'permissions:get': unknownPermissions,
  'talk:state': talkIdle,
  'talk:cancel': talkIdle,
  'talk:history': [],
  'talk:dictionary': [],
  'speech:status': { ...speechNone, configured: false },
  'ctrl:runs': [],
  'packages:list': {
    packages: [],
    index: null,
    available: [],
    reclaimable: { dirs: 0, blobs: 0, bytes: 0 },
    pendingRestart: [],
  },
};
