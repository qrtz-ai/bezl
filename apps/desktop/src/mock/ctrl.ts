// The mocked backend's ctrl do (e2e and `dev:mock`, ADR 0214): canned runs of the same shape crates/qrtz-ctrl
// produces, in memory. Ids come from one append counter (`do-0`, `run-1`). A command naming a site ("open youtube",
// "go to apple.com") runs through safari and finishes `done`; "send" stops at the confirm gate until `ctrl:confirm`
// approves (acts once) or denies; anything else asks which of three moves was meant. `ctrl:run` folds the first
// `at` records. The real chain over fixture trees, the gate, the stop key and replay are covered by
// `cargo test -p qrtz-ctrl`.
import {
  AppError,
  type CtrlAction,
  type CtrlAsk,
  type CtrlConfirm,
  type CtrlDecision,
  type CtrlModelState,
  type CtrlRunStatus,
  type CtrlRunSummary,
  type CtrlRunView,
  type CtrlStop,
  type Handlers,
} from '@qrtz/core';

/** One record: its kind, and what it changes in the run's view. */
interface Rec {
  kind: string;
  decision?: CtrlDecision;
  action?: CtrlAction;
  app?: string;
  ask?: CtrlAsk | null;
  confirm?: CtrlConfirm | null;
  finished?: CtrlRunStatus;
  stop?: CtrlStop;
}

interface Run {
  runId: string;
  doId: string;
  text: string;
  startedAt: number;
  recs: Rec[];
}

const decision = (
  id: string,
  node: CtrlDecision['node'],
  question: string,
  chosen: string,
  chosenLabel: string,
  p: number,
  outcome: CtrlDecision['outcome'] = 'act',
): CtrlDecision => ({
  id,
  node,
  question,
  chosen,
  chosenLabel,
  p,
  outcome,
  answered: false,
  backtracks: 0,
  model: 'qrtz-heuristic 2',
  options: 7,
});
const done = (
  id: string,
  kind: CtrlAction['kind'],
  label: string,
): CtrlAction => ({
  id,
  kind,
  label,
  status: 'done',
  err: null,
});

const SITES = ['youtube', 'gmail', 'github', 'reddit', 'wikipedia', 'netflix'];
const site = (text: string): string | null => {
  const words = text.toLowerCase().split(/\s+/);
  const domain = words.find((w) =>
    /^[a-z0-9-]+(\.[a-z0-9-]+)*\.[a-z]{2,6}$/.test(w),
  );
  if (domain) return domain;
  const s = words.find((w) => SITES.includes(w));
  return s ? `${s}.com` : null;
};

export function ctrlHandlers(now = () => Date.now()): Partial<Handlers> {
  let seq = 0;
  const runs = new Map<string, Run>();
  const order: string[] = [];
  let model: CtrlModelState = {
    consented: false,
    state: 'off',
    // ADR 0129: laya-coreml @ 7b8d7a2, e8 L512 bucket + tokenizer.json.
    sizeBytes: 483_924_446,
    bytesOnDisk: 0,
    scorer: 'heuristic',
    // QRT-293: mirrors the backend's model status (the mock's own runs are unchanged; the renderer is foo's).
    status: 'not_downloaded',
    error: null,
  };

  const get = (runId: string) => {
    const r = runs.get(runId);
    if (!r) throw new AppError('E_NOT_FOUND', 'that run is gone');
    return r;
  };
  const push = (r: Run, kind: string, patch: Omit<Rec, 'kind'> = {}) =>
    r.recs.push({ kind, ...patch });

  const view = (r: Run, at?: number): CtrlRunView => {
    const n = at === undefined ? r.recs.length : Math.min(at, r.recs.length);
    const decisions: CtrlDecision[] = [];
    const actions: CtrlAction[] = [];
    let app: string | null = null;
    let ask: CtrlAsk | null = null;
    let confirm: CtrlConfirm | null = null;
    let finished: CtrlRunStatus | null = null;
    let stop: CtrlStop | null = null;
    for (const rec of r.recs.slice(0, n)) {
      if (rec.decision) {
        const i = decisions.findIndex((d) => d.id === rec.decision!.id);
        if (i >= 0) decisions[i] = rec.decision;
        else decisions.push(rec.decision);
      }
      if (rec.action) actions.push(rec.action);
      if (rec.app) app = rec.app;
      if (rec.ask !== undefined) ask = rec.ask;
      if (rec.confirm !== undefined) confirm = rec.confirm;
      if (rec.finished) {
        finished = rec.finished;
        ask = null;
        confirm = null;
      }
      if (rec.stop) stop = rec.stop;
    }
    const status: CtrlRunStatus =
      finished ?? (ask ? 'asking' : confirm ? 'needs-confirm' : 'running');
    return {
      runId: r.runId,
      doId: r.doId,
      text: r.text,
      status,
      stop,
      app,
      startedAt: r.startedAt,
      at: n,
      total: r.recs.length,
      events: r.recs.map((x, i) => ({ seq: i, kind: x.kind })),
      decisions,
      actions,
      ask,
      confirm,
      backtracks: 0,
    };
  };
  const summary = (r: Run): CtrlRunSummary => {
    const v = view(r);
    return {
      runId: r.runId,
      doId: r.doId,
      text: r.text,
      status: v.status,
      startedAt: r.startedAt,
      actions: v.actions.length,
    };
  };
  const openApp = (r: Run, bundle: string, name: string) => {
    push(r, 'obs.apps');
    push(r, 'decision.made', {
      decision: decision(
        'd0',
        'choose-app',
        'which app should do it?',
        `app.${bundle}`,
        `open ${name}`,
        0.93,
      ),
    });
    push(r, 'action.requested');
    push(r, 'action.executed', {
      action: done('a0', 'open_app', `open ${name}`),
      app: name,
    });
    push(r, 'obs.tree');
    push(r, 'decision.made', {
      decision: decision(
        'd1',
        'verify',
        'is it done?',
        'verify.continue',
        'keep going',
        0.91,
      ),
    });
    push(r, 'obs.shortcuts');
  };
  const finish = (r: Run, status: CtrlRunStatus, stop?: CtrlStop) =>
    push(r, 'task.finished', { finished: status, ...(stop ? { stop } : {}) });

  /** The canned chains. */
  const play = (r: Run) => {
    const t = r.text.toLowerCase();
    const s = site(t);
    if (s) {
      openApp(r, 'com.apple.Safari', 'safari');
      push(r, 'decision.made', {
        decision: decision(
          'd2',
          'choose-move',
          'what should ctrl do next?',
          'key.cmd+l',
          'focus the address bar (cmd+l)',
          0.88,
        ),
      });
      push(r, 'action.executed', {
        action: done('a1', 'key_combo', 'press cmd+l'),
      });
      push(r, 'obs.tree');
      push(r, 'decision.made', {
        decision: decision(
          'd3',
          'choose-text',
          'what should ctrl type?',
          'text.0.go',
          `type “${s}” and press return`,
          0.92,
        ),
      });
      push(r, 'action.executed', {
        action: done('a2', 'set_value', `type “${s}” and press return`),
      });
      push(r, 'obs.tree');
      push(r, 'decision.made', {
        decision: decision(
          'd4',
          'verify',
          'is it done?',
          'verify.done',
          "it's done",
          0.93,
        ),
      });
      finish(r, 'done');
      return;
    }
    if (/\bsend\b/.test(t)) {
      openApp(r, 'com.apple.mail', 'mail');
      push(r, 'decision.made', {
        decision: decision(
          'd2',
          'choose-move',
          'what should ctrl do next?',
          'menu.m:50:1',
          'send (menu)',
          0.74,
          'tentative',
        ),
      });
      push(r, 'confirm.requested', {
        confirm: {
          gateId: 'g0',
          rules: ['label:send'],
          action: 'press send',
          app: 'mail',
        },
      });
      return;
    }
    openApp(r, 'com.tinyspeck.slackmacgap', 'slack');
    push(r, 'decision.made', {
      decision: decision(
        'd2',
        'choose-move',
        'what should ctrl do next?',
        'none',
        'none of these',
        0.31,
        'ask',
      ),
      ask: {
        decisionId: 'd2',
        node: 'choose-move',
        question: 'what should ctrl do next?',
        options: [
          {
            id: 'menu.m:60:1',
            label: 'switch workspace… (menu)',
            p: 0.24,
            gated: false,
          },
          {
            id: 'branch.e:60:2',
            label: 'look in web area',
            p: 0.19,
            gated: false,
          },
          { id: 'key.cmd+f', label: 'find (cmd+f)', p: 0.12, gated: false },
        ],
      },
    });
  };

  const waiting = () =>
    order
      .map((id) => runs.get(id)!)
      .filter((r) => {
        const v = view(r);
        return v.status === 'asking' || v.status === 'needs-confirm';
      });

  return {
    'ctrl:do': ({ text }) => {
      const clean = text.trim().replace(/\s+/g, ' ');
      for (const r of waiting()) finish(r, 'cancelled');
      const doId = `do-${seq++}`;
      const r: Run = {
        runId: `run-${seq++}`,
        doId,
        text: clean,
        startedAt: now(),
        recs: [],
      };
      runs.set(r.runId, r);
      order.push(r.runId);
      push(r, 'do.started');
      play(r);
      return view(r);
    },
    'ctrl:runs': () => [...order].reverse().map((id) => summary(runs.get(id)!)),
    'ctrl:run': ({ runId, at }) => view(get(runId), at),
    'ctrl:answer': ({ runId, decisionId, optionId }) => {
      const r = get(runId);
      const v = view(r);
      if (!v.ask || v.ask.decisionId !== decisionId)
        throw new AppError(
          'E_VALIDATION',
          'that question was already answered',
        );
      const picked = v.ask.options.find((o) => o.id === optionId);
      if (optionId !== null && !picked)
        throw new AppError('E_VALIDATION', "that isn't one of the choices");
      if (!picked) {
        push(r, 'decision.answered_by_user', { ask: null });
        finish(r, 'cancelled');
        return view(r);
      }
      const d = v.decisions.find((x) => x.id === decisionId)!;
      push(r, 'decision.answered_by_user', {
        ask: null,
        decision: {
          ...d,
          chosen: picked.id,
          chosenLabel: picked.label,
          p: picked.p,
          answered: true,
        },
      });
      push(r, 'action.executed', { action: done('a1', 'press', picked.label) });
      push(r, 'obs.tree');
      push(r, 'decision.made', {
        decision: decision(
          'd3',
          'verify',
          'is it done?',
          'verify.done',
          "it's done",
          0.86,
        ),
      });
      finish(r, 'done');
      return view(r);
    },
    'ctrl:confirm': ({ runId, gateId, answer }) => {
      const r = get(runId);
      const v = view(r);
      if (!v.confirm || v.confirm.gateId !== gateId)
        throw new AppError('E_VALIDATION', 'that request was already answered');
      push(r, 'confirm.resolved', { confirm: null });
      if (answer === 'deny') {
        finish(r, 'cancelled');
        return view(r);
      }
      push(r, 'obs.tree');
      push(r, 'confirm.reverified');
      push(r, 'action.executed', { action: done('a1', 'press', 'press send') });
      push(r, 'obs.tree');
      push(r, 'decision.made', {
        decision: decision(
          'd3',
          'verify',
          'is it done?',
          'verify.done',
          "it's done",
          0.9,
        ),
      });
      finish(r, 'done');
      return view(r);
    },
    'ctrl:stop': ({ runId }) => {
      const r = get(runId);
      if (
        !view(r).status.match(
          /^(done|failed|cancelled|asked-out|stopped|out-of-steps)$/,
        )
      )
        finish(r, 'stopped', 'stop-button');
      return { runId, status: view(r).status };
    },
    // ADR 0116 / 0129: the decision model. The mock never goes online: a yes "installs" the pinned model at once
    // (the Rust backend downloads and checks it), and the browser has no Core ML, so the built-in rules still
    // decide, like any build without the runtime.
    'ctrl:model': () => ({ ...model }),
    'ctrl:model-consent': ({ consent }) => {
      model = consent
        ? {
            ...model,
            consented: true,
            state: 'installed',
            bytesOnDisk: model.sizeBytes,
            // Like a build without the Core ML runtime: installed, but nothing can run it.
            status: 'unavailable',
            error: null,
          }
        : {
            ...model,
            consented: false,
            state: 'off',
            status: 'not_downloaded',
            error: null,
            bytesOnDisk: 0,
          };
      return { ...model };
    },
  };
}
