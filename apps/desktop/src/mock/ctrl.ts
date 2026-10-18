// The mocked backend's ctrl (e2e and `dev:mock`): the same store semantics as crates/qrtz-ctrl `store.rs`, in
// memory. Ids come from one append counter (`task-0`, `run-1`); a run records `task.started`, `intent.pending`,
// `obs.apps`, then canned decision-layer chains of the same shape the real layers produce (ADR 0106): "tabs"
// → close tabs, "downloads" / "folder" → clean up a folder, anything else asks which intent. `ctrl:run` folds the
// first `at` events. The real layers, persistence and replay are covered by `cargo test -p qrtz-ctrl`.
// The mock acts as if the ctrl helper were connected (ADR 0108): a close-tabs plan executes in the background
// (one guarded action, verified) and finishes `done`; a folder cleanup stays `planned` (file routes are slice 7).
// The real executor against a fake helper is covered by `crates/qrtz-ctrl/tests/exec.rs`.
// "quit" → quit textedit, irreversible: the run stops at the confirm gate (ADR 0111) with two windows (one unsaved)
// until `ctrl:confirm` approves (acts once), remembers (a standing approval: the next run doesn't ask) or denies.
// The real gate is covered by `crates/qrtz-ctrl/tests/gate.rs`.
import {
  AppError,
  type CtrlConfirm,
  type CtrlDecision,
  type CtrlModelState,
  type CtrlOption,
  type CtrlPlan,
  type CtrlRun,
  type CtrlRunState,
  type CtrlRunView,
  type CtrlTask,
  type Handlers,
} from '@qrtz/core';

type Row = CtrlRunView['events'][number];
interface Rec {
  row: Row;
  decision?: CtrlDecision;
  plan?: CtrlPlan;
  answer?: { decisionId: string; choice: string | null };
  finished?: 'cancelled' | 'done';
  confirm?: CtrlConfirm;
  answerGate?: NonNullable<CtrlConfirm['answer']>;
  reverified?: true;
  action?: { id: string; status: 'executing' | 'done' };
  verified?: { actionId: string; outcome: 'passed' };
  held?: 'files-not-yet';
  /** A folder cleanup's batch (ADR 0117): its files, and which of them moved since (a "renamed" task). */
  batch?: { files: string[]; moved: string[] };
  undone?: number;
}

const opt = (id: string, label: string, p: number): CtrlOption => ({
  id,
  label,
  p,
  irreversible: false,
  refusedInBackground: false,
});
const NONE = opt('none', 'none of these', 0.01);
const act = (
  node: CtrlDecision['node'],
  options: CtrlOption[],
): Omit<CtrlDecision, 'id'> => ({
  node,
  outcome: 'act',
  answered: false,
  chosen: options[0]!.id,
  chosenLabel: options[0]!.label,
  p: options[0]!.p,
  model: 'qrtz-heuristic 1',
  options: [...options, NONE],
});

const INTENTS = {
  close_tabs: opt('intent.close_tabs', 'close browser tabs', 0.07),
  clean_folder: opt('intent.clean_folder', 'clean up a folder', 0.07),
  trash_old: opt('intent.clean_folder', 'clean up a folder', 0.07),
  close_windows: opt('intent.close_windows', 'close windows', 0.07),
  empty_trash: {
    ...opt('intent.empty_trash', 'empty the trash', 0.07),
    irreversible: true,
  },
  quit_app: {
    ...opt('intent.quit_app', 'quit an app', 0.07),
    irreversible: true,
  },
};
type Intent =
  | 'close_tabs'
  | 'clean_folder'
  | 'trash_old'
  | 'empty_trash'
  | 'quit_app';
/** What an unclear task is asked (the three most likely intents). */
const ASKED = [INTENTS.close_tabs, INTENTS.clean_folder, INTENTS.close_windows];

/** After the intent: the rest of the chain, all acting. */
const REST: Record<Intent, Omit<CtrlDecision, 'id'>[]> = {
  close_tabs: [
    act('choose_app', [
      opt('app.default_browser', 'your default browser', 0.88),
      opt('app.com.google.Chrome', 'google chrome', 0.04),
    ]),
    act('choose_target', [
      opt('scope.all_tabs', 'all tabs', 0.99),
      opt('scope.front_tab', 'front tab', 0.0),
    ]),
    act('choose_move', [
      opt('route.scripting.close_all_tabs', 'close every tab by script', 0.94),
      opt('shortcut.cmd+w', 'cmd-w close tab', 0.02),
    ]),
  ],
  quit_app: [
    act('choose_app', [
      opt('app.com.apple.TextEdit', 'textedit', 0.9),
      opt('app.com.apple.Notes', 'notes', 0.03),
    ]),
    act('choose_move', [
      { ...opt('shortcut.cmd+q', 'quit', 0.95), irreversible: true },
    ]),
  ],
  clean_folder: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_target', [
      opt('folder.downloads', 'downloads', 0.98),
      opt('folder.desktop', 'desktop', 0.01),
    ]),
    act('choose_move', [
      opt('route.files.tidy', 'sort by kind, nothing deleted', 0.89),
      opt('route.files.sort_by_kind', 'sort files into folders by kind', 0.03),
    ]),
  ],
  // ADR 0122: emptying the Trash deletes through the file executor (never the finder), always behind the card.
  empty_trash: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_move', [
      {
        ...opt('route.files.empty_trash', 'empty the trash', 0.9),
        irreversible: true,
      },
      {
        ...opt(
          'shortcut.cmd+shift+delete',
          'cmd-shift-delete empty trash',
          0.05,
        ),
        irreversible: true,
      },
    ]),
  ],
  trash_old: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_target', [
      opt('folder.downloads', 'downloads', 0.98),
      opt('folder.desktop', 'desktop', 0.01),
    ]),
    act('choose_move', [
      {
        ...opt(
          'route.files.trash_old',
          'move old installers to the trash',
          0.9,
        ),
        irreversible: true,
      },
      opt('route.files.tidy', 'sort by kind, nothing deleted', 0.05),
    ]),
  ],
};

const intentOf = (text: string): Intent | null =>
  /\bquit\b/i.test(text)
    ? 'quit_app'
    : /\bempty\b/i.test(text) && /\b(trash|bin)\b/i.test(text)
      ? 'empty_trash'
      : /\btabs?\b/i.test(text)
        ? 'close_tabs'
        : /download|folder/i.test(text)
          ? /\b(delete|trash|old)\b/i.test(text)
            ? 'trash_old'
            : 'clean_folder'
          : null;

const PREVIEW: CtrlConfirm['items'] = [
  { key: '3f2a9c1d0b7e#0', kind: 'window', label: 'notes.txt', unsaved: false },
  { key: '8c1e44a0d2f9#0', kind: 'window', label: 'draft.txt', unsaved: true },
];

/** ADR 0119: tabs held because the browser blocks ctrl's page script (a task that says "blocked"): can't tell. */
const BLIND: CtrlConfirm['items'] = [
  { key: '5d1c0a9e8f7b#0', kind: 'tab', label: 'inbox', unsaved: null },
  { key: 'a7b8c9d0e1f2#0', kind: 'tab', label: 'expense form', unsaved: null },
];

/** ADR 0122: what an empty_trash run would delete (the Finder's own files and anything just trashed are left). */
const TRASH: CtrlConfirm['items'] = [
  { key: 'f0c1d2e3f4a5', kind: 'file', label: 'old folder', unsaved: false },
  { key: 'f5a4f3e2d1c0', kind: 'file', label: 'old.dmg', unsaved: false },
];

/** Slice 7 (ADR 0113): the old installers a trash_old run would send to the Trash. */
const FILES: CtrlConfirm['items'] = [
  { key: 'f1a2b3c4d5e6', kind: 'file', label: 'ancient.pkg', unsaved: false },
  { key: 'f6e5d4c3b2a1', kind: 'file', label: 'oldapp.dmg', unsaved: false },
];

export function ctrlHandlers(now = () => Date.now()): Partial<Handlers> {
  let seq = 0;
  const tasks: CtrlTask[] = [];
  const runs = new Map<string, { run: CtrlRun; recs: Rec[] }>();
  const task = (id: string) => {
    const t = tasks.find((x) => x.id === id);
    if (!t) throw new AppError('E_NOT_FOUND', 'that task or run is gone');
    return t;
  };
  const stateAt = (recs: Rec[], at: number): CtrlRunState => {
    const decisions: CtrlDecision[] = [];
    let plan: CtrlPlan | null = null;
    let ask: CtrlRunState['ask'] = null;
    let pending = false;
    let finished: Rec['finished'] | null = null;
    const actions = new Map<string, 'executing' | 'done'>();
    const verified: NonNullable<Rec['verified']>[] = [];
    let held: Rec['held'] | null = null;
    let confirm: CtrlConfirm | null = null;
    let batch: Rec['batch'] | null = null;
    let undone: number | null = null;
    for (const r of recs.slice(0, at)) {
      if (r.batch) batch = r.batch;
      if (r.undone !== undefined) undone = r.undone;
      if (r.confirm) confirm = r.confirm;
      if (r.answerGate && confirm)
        confirm = { ...confirm, answer: r.answerGate };
      if (r.reverified && confirm) confirm = { ...confirm, reverified: true };
      if (r.action && confirm) confirm = { ...confirm, used: true };
      if (r.held) held = r.held;
      if (r.action) actions.set(r.action.id, r.action.status);
      if (r.verified) verified.push(r.verified);
      if (r.row.kind === 'intent.pending') pending = true;
      if (r.decision) {
        decisions.push(r.decision);
        if (r.decision.outcome === 'ask')
          ask = {
            decisionId: r.decision.id,
            node: r.decision.node,
            options: r.decision.options.filter((o) => o.id !== 'none'),
          };
      }
      if (r.answer) {
        ask = null;
        const d = decisions.find((x) => x.id === r.answer!.decisionId);
        const c = d?.options.find((o) => o.id === r.answer!.choice);
        if (d && c) {
          const i = decisions.indexOf(d);
          decisions[i] = {
            ...d,
            answered: true,
            chosen: c.id,
            chosenLabel: c.label,
          };
        }
      }
      if (r.plan) {
        plan = r.plan;
        pending = false;
      }
      if (r.finished) finished = r.finished;
    }
    const status = finished
      ? finished
      : ask
        ? 'asking'
        : confirm && !confirm.answer
          ? 'needs-confirm'
          : pending
            ? 'pending-intent'
            : plan && !actions.size
              ? 'planned'
              : 'running';
    return {
      actions: [...actions].map(([id, st]) => ({ id, status: st })),
      verified,
      held,
      violations: 0,
      inconclusive: 0,
      quarantined: [],
      status,
      decisions,
      plan,
      ask,
      confirm,
      undo:
        batch && finished === 'done'
          ? {
              actionId: 'a0',
              files: batch.files.length,
              ready:
                undone !== null || at < recs.length
                  ? 0
                  : batch.files.length - batch.moved.length,
              cantUndo: batch.moved.map((f) => ({
                key: `f-${f}`,
                label: f,
                why: 'moved' as const,
              })),
              done: undone === null ? null : { undone },
            }
          : null,
    };
  };
  const push = (
    r: { run: CtrlRun; recs: Rec[] },
    kind: string,
    extra: Omit<Rec, 'row'> = {},
    node: string | null = null,
  ) => {
    r.recs.push({
      row: { seq: r.recs.length, kind, at: now(), actionId: null, node },
      ...extra,
    });
  };
  const decide = (
    r: { run: CtrlRun; recs: Rec[] },
    d: Omit<CtrlDecision, 'id'>,
  ) => {
    const n = r.recs.filter((x) => x.decision).length;
    push(r, 'decision.made', { decision: { ...d, id: `d${n}` } }, d.node);
  };
  const execute = (r: { run: CtrlRun; recs: Rec[] }) => {
    // The executor (ADR 0108): one guarded action, then its verify step.
    const a = (kind: string, extra: Omit<Rec, 'row'> = {}) =>
      r.recs.push({
        row: {
          seq: r.recs.length,
          kind,
          at: now(),
          actionId: 'a0',
          node: null,
        },
        ...extra,
      });
    a('action.requested', { action: { id: 'a0', status: 'executing' } });
    a('action.decided');
    a('guard.snapshot');
    a('guard.snapshot');
    a('action.executed', { action: { id: 'a0', status: 'done' } });
    a('guard.result');
    a('verify.result', { verified: { actionId: 'a0', outcome: 'passed' } });
    push(r, 'task.finished', { finished: 'done' });
  };
  const finishChain = (r: { run: CtrlRun; recs: Rec[] }, intent: Intent) => {
    for (const d of REST[intent]) decide(r, d);
    const s = stateAt(r.recs, r.recs.length);
    push(r, 'plan.produced', {
      plan: {
        irreversible:
          intent === 'quit_app' ||
          intent === 'trash_old' ||
          intent === 'empty_trash',
        steps: s.decisions.map((d) => ({
          node: d.node,
          option: d.chosen,
          label: d.chosenLabel,
        })),
      },
    });
    if (
      intent === 'close_tabs' &&
      /\bblocked\b/i.test(task(r.run.taskId).text)
    ) {
      // ADR 0114: the tabs ctrl can't read are held behind the card (the plain ones would have closed already).
      push(r, 'confirm.requested', {
        confirm: {
          gateId: `g${r.recs.filter((x) => x.confirm).length}`,
          action: 'close browser tabs',
          app: 'safari',
          move: 'close tabs with unsaved input',
          items: BLIND,
          complete: true,
          rules: ['choose_move:route.scripting.close_unsaved_tabs'],
          answer: null,
          reverified: null,
          used: false,
        },
      });
      return;
    }
    if (
      intent === 'quit_app' ||
      intent === 'trash_old' ||
      intent === 'empty_trash'
    ) {
      // The confirm gate (ADR 0111): a standing approval covers it, or the run waits for the user.
      const g = r.recs.filter((x) => x.confirm).length;
      const quit = intent === 'quit_app';
      const empty = intent === 'empty_trash';
      push(r, 'confirm.requested', {
        confirm: {
          gateId: `g${g}`,
          action: quit
            ? 'quit an app'
            : empty
              ? 'empty the trash'
              : 'clean up a folder',
          app: quit ? 'textedit' : 'files (no app)',
          move: quit
            ? 'quit'
            : empty
              ? 'empty the trash'
              : 'move old installers to the trash',
          items: quit ? PREVIEW : empty ? TRASH : FILES,
          complete: true,
          rules: quit
            ? ['choose_intent:intent.quit_app', 'choose_move:shortcut.cmd+q']
            : empty
              ? [
                  'choose_intent:intent.empty_trash',
                  'choose_move:route.files.empty_trash',
                ]
              : ['choose_move:route.files.trash_old'],
          answer: null,
          reverified: null,
          used: false,
        },
      });
      if (task(r.run.taskId).approvals?.length) {
        push(r, 'confirm.resolved', { answerGate: 'standing' });
        push(r, 'confirm.reverified', { reverified: true });
        execute(r);
      }
      return;
    }
    // Slice 7 (ADR 0113): a folder cleanup is one batch of file moves, guarded and verified like any action.
    execute(r);
    if (intent !== 'clean_folder') return;
    const moved = /\brenamed\b/i.test(task(r.run.taskId).text)
      ? ['photo.png']
      : [];
    r.recs.find((x) => x.row.kind === 'action.requested')!.batch = {
      files: ['photo.png', 'taxes 2025.pdf', 'site.zip'],
      moved,
    };
  };
  const sync = (r: { run: CtrlRun; recs: Rec[] }) => {
    r.run.events = r.recs.length;
    r.run.status = stateAt(r.recs, r.recs.length).status;
  };
  const view = (r: { run: CtrlRun; recs: Rec[] }, at?: number) => {
    const n = Math.min(at ?? r.recs.length, r.recs.length);
    return {
      run: r.run,
      events: r.recs.map((x) => x.row),
      at: n,
      state: stateAt(r.recs, n),
    };
  };
  let model: CtrlModelState = {
    consented: false,
    state: 'off',
    sizeBytes: 0,
    bytesOnDisk: 0,
    scorer: 'heuristic',
    error: null,
  };
  const get = (runId: string) => {
    const r = runs.get(runId);
    if (!r) throw new AppError('E_NOT_FOUND', 'that task or run is gone');
    return r;
  };
  return {
    'ctrl:tasks': () => [...tasks].reverse(),
    'ctrl:create-task': ({ text, schedule }) => {
      const t: CtrlTask = {
        id: `task-${seq++}`,
        text: text.trim(),
        schedule,
        paused: false,
        createdAt: now(),
        runs: 0,
        lastRunAt: null,
        // Mock has no ticker; ASAP shows as due now, cadences as null (real nextRunAt is server-side).
        nextRunAt: schedule.kind === 'once' ? (schedule.at ?? now()) : null,
        approvals: [],
      };
      tasks.push(t);
      return t;
    },
    'ctrl:set-paused': ({ id, paused }) => {
      const t = task(id);
      if (t.paused !== paused) seq++;
      t.paused = paused;
      if (paused) t.nextRunAt = null;
      return t;
    },
    'ctrl:delete-task': ({ id }) => {
      tasks.splice(tasks.indexOf(task(id)), 1);
      seq++;
      return { ok: true as const };
    },
    'ctrl:run-now': ({ id }) => {
      const t = task(id);
      const at = now();
      const r = {
        run: {
          id: `run-${seq++}`,
          taskId: id,
          at,
          status: 'pending-intent',
          events: 0,
          violations: 0,
        } as CtrlRun,
        recs: [] as Rec[],
      };
      push(r, 'task.started');
      push(r, 'intent.pending');
      push(r, 'obs.apps');
      const intent = intentOf(t.text);
      if (intent) {
        decide(r, act('choose_intent', [{ ...INTENTS[intent], p: 0.93 }]));
        finishChain(r, intent);
      } else {
        decide(r, {
          ...act('choose_intent', ASKED),
          outcome: 'ask',
          chosen: 'none',
          chosenLabel: 'none of these',
          p: 0.2,
        });
      }
      sync(r);
      runs.set(r.run.id, r);
      t.runs += 1;
      t.lastRunAt = at;
      if (t.schedule.kind === 'once') t.nextRunAt = null;
      return r.run;
    },
    'ctrl:runs': ({ taskId }) =>
      [...runs.values()]
        .filter((r) => r.run.taskId === task(taskId).id)
        .map((r) => r.run)
        .reverse(),
    'ctrl:run': ({ runId, at }) => view(get(runId), at),
    'ctrl:answer': ({ runId, decisionId, optionId }) => {
      const r = get(runId);
      const s = stateAt(r.recs, r.recs.length);
      if (s.ask?.decisionId !== decisionId)
        throw new AppError(
          'E_VALIDATION',
          'that question was already answered',
        );
      if (optionId !== null && !s.ask.options.some((o) => o.id === optionId))
        throw new AppError('E_VALIDATION', 'pick one of the options');
      push(r, 'decision.answered_by_user', {
        answer: { decisionId, choice: optionId },
      });
      if (optionId === null)
        push(r, 'task.finished', { finished: 'cancelled' });
      else {
        const intent = optionId.replace('intent.', '') as Intent;
        if (intent in REST) finishChain(r, intent);
      }
      sync(r);
      return view(r);
    },
    'ctrl:confirm': ({ runId, gateId, answer }) => {
      const r = get(runId);
      const c = stateAt(r.recs, r.recs.length).confirm;
      if (!c || c.gateId !== gateId || c.answer)
        throw new AppError('E_VALIDATION', 'that request was already answered');
      if (answer === 'deny') {
        push(r, 'confirm.resolved', { answerGate: 'denied' });
        push(r, 'task.finished', { finished: 'cancelled' });
      } else {
        if (answer === 'remember') {
          const t = task(r.run.taskId);
          t.approvals = [
            ...(t.approvals ?? []),
            {
              id: `approval-${seq++}`,
              intent: 'intent.quit_app',
              move: 'shortcut.cmd+q',
              app: 'com.apple.TextEdit',
              items: PREVIEW.length,
              unsaved: 1,
              grantedAt: now(),
            },
          ];
        }
        push(r, 'confirm.resolved', {
          answerGate: answer === 'remember' ? 'remembered' : 'approved',
        });
        push(r, 'confirm.reverified', { reverified: true });
        execute(r);
      }
      sync(r);
      return view(r);
    },
    // ADR 0116: the decision model. The pinned manifest is still a placeholder, so a yes is recorded and the
    // download refused (no network), exactly like the Rust backend.
    'ctrl:model': () => ({ ...model }),
    'ctrl:model-consent': ({ consent }) => {
      model = consent
        ? {
            ...model,
            consented: true,
            state: 'unavailable',
            error: 'laya.placeholder',
          }
        : {
            ...model,
            consented: false,
            state: 'off',
            error: null,
            bytesOnDisk: 0,
          };
      return { ...model };
    },
    // ADR 0117: put a finished cleanup back. Files that moved since stay; without `rest` that refuses.
    'ctrl:undo': ({ runId, rest }) => {
      const r = get(runId);
      const u = stateAt(r.recs, r.recs.length).undo;
      if (!u)
        throw new AppError('E_VALIDATION', "this run didn't move any files");
      if (u.done)
        throw new AppError('E_VALIDATION', 'this run was already put back');
      const row = (kind: string, extra: Omit<Rec, 'row'> = {}) =>
        r.recs.push({
          row: {
            seq: r.recs.length,
            kind,
            at: now(),
            actionId: 'a0',
            node: null,
          },
          ...extra,
        });
      if (u.cantUndo.length && !rest) row('undo.refused');
      else row('undo.done', { undone: u.ready });
      sync(r);
      return view(r);
    },
    'ctrl:revoke-approval': ({ taskId, approvalId }) => {
      const t = task(taskId);
      if (!t.approvals?.some((a) => a.id === approvalId))
        throw new AppError('E_NOT_FOUND', 'that task or run is gone');
      t.approvals = t.approvals.filter((a) => a.id !== approvalId);
      seq++;
      return t;
    },
  };
}
