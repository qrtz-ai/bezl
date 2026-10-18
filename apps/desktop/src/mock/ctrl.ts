// The mocked backend's ctrl (e2e and `dev:mock`): the same store semantics as crates/qrtz-ctrl `store.rs`, in
// memory. Ids come from one append counter (`task-0`, `run-1`); a run records `task.started`, `intent.pending`,
// `obs.apps`, then canned decision-layer chains of the same shape the real layers produce (ADR 0106): "tabs"
// → close tabs, "downloads" / "folder" → clean up a folder, anything else asks which intent. `ctrl:run` folds the
// first `at` events. The real layers, persistence and replay are covered by `cargo test -p qrtz-ctrl`.
import {
  AppError,
  type CtrlDecision,
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
  finished?: 'cancelled';
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
  close_windows: opt('intent.close_windows', 'close windows', 0.07),
};
type Intent = 'close_tabs' | 'clean_folder';

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
      opt('route.files.tidy', 'sort by kind, trash old installers', 0.89),
      opt('route.files.sort_by_kind', 'sort files into folders by kind', 0.03),
    ]),
  ],
};

const intentOf = (text: string): Intent | null =>
  /\btabs?\b/i.test(text)
    ? 'close_tabs'
    : /download|folder/i.test(text)
      ? 'clean_folder'
      : null;

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
    let finished = false;
    for (const r of recs.slice(0, at)) {
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
      if (r.finished) finished = true;
    }
    const status = finished
      ? 'cancelled'
      : ask
        ? 'asking'
        : pending
          ? 'pending-intent'
          : plan
            ? 'planned'
            : 'running';
    return {
      actions: [],
      violations: 0,
      inconclusive: 0,
      quarantined: [],
      status,
      decisions,
      plan,
      ask,
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
  const finishChain = (r: { run: CtrlRun; recs: Rec[] }, intent: Intent) => {
    for (const d of REST[intent]) decide(r, d);
    const s = stateAt(r.recs, r.recs.length);
    push(r, 'plan.produced', {
      plan: {
        irreversible: false,
        steps: s.decisions.map((d) => ({
          node: d.node,
          option: d.chosen,
          label: d.chosenLabel,
        })),
      },
    });
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
          ...act('choose_intent', Object.values(INTENTS)),
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
  };
}
