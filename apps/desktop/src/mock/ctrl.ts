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
  finished?: 'cancelled' | 'done' | 'asked-out';
  confirm?: CtrlConfirm;
  answerGate?: NonNullable<CtrlConfirm['answer']>;
  reverified?: true;
  action?: { id: string; status: 'executing' | 'done' };
  verified?: { actionId: string; outcome: 'passed' };
  held?: 'files-not-yet' | 'folder-not-yet';
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
  tidy_documents: opt('intent.clean_folder', 'clean up a folder', 0.07),
  tidy_desktop: opt('intent.clean_folder', 'clean up a folder', 0.07),
  open_calendar: opt('intent.open_app', 'open an app', 0.07),
  close_windows: opt('intent.close_windows', 'close windows', 0.07),
  empty_trash: {
    ...opt('intent.empty_trash', 'empty the trash', 0.07),
    irreversible: true,
  },
  quit_app: {
    ...opt('intent.quit_app', 'quit an app', 0.07),
    irreversible: true,
  },
  quit_unused: {
    ...opt('intent.quit_unused', "quit apps you haven't used today", 0.07),
    irreversible: true,
  },
  hide_others: opt('intent.hide_others', 'hide your other apps', 0.07),
  mute: opt('intent.mute', 'mute your mac', 0.07),
  unmute: opt('intent.unmute', 'unmute your mac', 0.07),
  set_volume: opt('intent.set_volume', 'set the volume', 0.07),
  volume_up: opt('intent.change_volume', 'turn the volume up or down', 0.07),
  volume_down: opt('intent.change_volume', 'turn the volume up or down', 0.07),
  dark_mode: opt('intent.dark_mode', 'switch dark mode', 0.07),
  light_mode: opt('intent.dark_mode', 'switch dark mode', 0.07),
  open_email: opt('intent.open_app', 'open an app', 0.07),
};
type Intent =
  | 'close_tabs'
  | 'clean_folder'
  | 'trash_old'
  | 'tidy_documents'
  | 'tidy_desktop'
  | 'open_calendar'
  | 'empty_trash'
  | 'quit_app'
  | 'quit_unused'
  | 'hide_others'
  | 'mute'
  | 'unmute'
  | 'set_volume'
  | 'volume_up'
  | 'volume_down'
  | 'dark_mode'
  | 'light_mode'
  | 'open_email';
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
  // ADR 0130: the apps not used today (the helper's activation record), quit behind the card; no app question.
  quit_unused: [
    act('choose_move', [
      {
        ...opt(
          'route.workspace.quit_unused',
          'quit them without bringing anything forward',
          0.97,
        ),
        irreversible: true,
      },
    ]),
  ],
  // ADR 0134: every running app but the frontmost one and qrtz, hidden; reversible, so no card and no app question.
  hide_others: [
    act('choose_move', [
      opt(
        'route.workspace.hide_others',
        "hide them, the app you're in stays",
        0.97,
      ),
    ]),
  ],
  // ADR 0137: the appearance, through system events (faceless); reversible, no card.
  dark_mode: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt('route.scripting.dark_mode_on', 'switch to dark mode', 0.9),
      opt('route.scripting.dark_mode_off', 'switch to light mode', 0.05),
    ]),
  ],
  light_mode: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt('route.scripting.dark_mode_off', 'switch to light mode', 0.9),
      opt('route.scripting.dark_mode_on', 'switch to dark mode', 0.05),
    ]),
  ],
  // ADR 0138: "my email" is mail, already running, so it's already open (nothing comes forward).
  open_email: [
    act('choose_app', [opt('app.com.apple.mail', 'mail', 0.95)]),
    act('choose_move', [
      opt('route.workspace.launch_hidden', 'start it hidden', 0.95),
    ]),
  ],
  // ADR 0140: the level is the task's own number (choose_target, added in finishChain); reversible, no card.
  set_volume: [
    act('choose_move', [
      opt(
        'route.system.set_volume',
        'set the sound output to that level',
        0.97,
      ),
    ]),
  ],
  // ADR 0143: a step from the level read just before (choose_target `step.N`, added in finishChain); no card.
  volume_up: [
    act('choose_move', [
      opt('route.system.volume_up', 'turn the sound output up a step', 0.97),
    ]),
  ],
  volume_down: [
    act('choose_move', [
      opt(
        'route.system.volume_down',
        'turn the sound output down a step',
        0.97,
      ),
    ]),
  ],
  // ADR 0139: the reverse of mute; reversible, no card, no app question.
  unmute: [
    act('choose_move', [
      opt('route.system.unmute', 'turn the sound output back on', 0.97),
    ]),
  ],
  // ADR 0135: the sound output, muted in the helper's own process; reversible, no card, no app question.
  mute: [
    act('choose_move', [
      opt('route.system.mute', 'mute the sound output', 0.97),
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
  // ADR 0125: the Documents folder has no routes yet; its one move is a hold that says so.
  tidy_documents: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_target', [
      opt('folder.documents', 'documents', 0.98),
      opt('folder.downloads', 'downloads', 0.01),
    ]),
    act('choose_move', [
      opt(
        'route.files.documents_hold',
        'leave documents as they are, not supported yet',
        0.97,
      ),
    ]),
  ],
  // ADR 0124 / 0126: only macOS's own screenshots and screen recordings move, into the folder screenshots are
  // saved to (desktop/screenshots unless the screenshot app saves them somewhere else); nothing is deleted.
  tidy_desktop: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_target', [
      opt('folder.desktop', 'desktop', 0.98),
      opt('folder.downloads', 'downloads', 0.01),
    ]),
    act('choose_move', [
      opt(
        'route.files.desktop_screenshots',
        'move screenshots into a folder, nothing deleted',
        0.95,
      ),
      {
        ...opt(
          'route.files.trash_screenshots',
          'move screenshots to the trash',
          0.02,
        ),
        irreversible: true,
      },
    ]),
  ],
  // ADR 0128: "my calendar" is calendar.app; a running one is already open (nothing launched, nothing hidden).
  open_calendar: [
    act('choose_app', [
      opt('app.com.apple.iCal', 'calendar', 0.95),
      opt('app.name.notion-calendar', 'notion calendar', 0.03),
    ]),
    act('choose_move', [
      opt('route.workspace.launch_hidden', 'start it hidden', 0.97),
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

/** ADR 0140: the level a volume task names (the number after "to" / "volume"), 0–100. */
const levelOf = (text: string): number | null => {
  // ADR 0143: "all the way up / down" is the end of the range.
  const end = /\ball the way (up|down)\b/i.exec(text);
  if (end) return end[1]?.toLowerCase() === 'up' ? 100 : 0;
  const m = /\b(?:to|volume)\s+(\d{1,3})\b/i.exec(text);
  const n = m ? Number(m[1]) : NaN;
  return n >= 0 && n <= 100 ? n : null;
};

/** ADR 0143: up / down from the words, or null (none or both). */
const directionOf = (text: string): 'volume_up' | 'volume_down' | null => {
  const up = /\b(up|louder|raise|increase|higher|boost)\b/i.test(text);
  const down = /\b(down|quieter|softer|lower|decrease|reduce)\b/i.test(text);
  return up === down ? null : up ? 'volume_up' : 'volume_down';
};

/** ADR 0143: the step of a relative change: "by N" / "up N" / "down N", "a bit" is 5, else 10. */
const stepOf = (text: string): number => {
  const m = /\b(?:by|up|down)\s+(\d{1,3})\b/i.exec(text);
  const n = m ? Number(m[1]) : NaN;
  if (n >= 1 && n <= 100) return n;
  return /\b(bit|little|slightly|tad)\b/i.test(text) ? 5 : 10;
};

const relativeOf = (text: string): Intent | null =>
  levelOf(text) === null &&
  (/\b(volume|sound|audio|louder|quieter|softer)\b/i.test(text) ||
    /\bturn\b.*\bit\b/i.test(text))
    ? directionOf(text)
    : null;

const intentOf = (text: string): Intent | null =>
  relativeOf(text) !== null
    ? relativeOf(text)
    : /\b(volume|sound)\b/i.test(text) && levelOf(text) !== null
      ? 'set_volume'
      : /\b(unmute|unsilence)\b/i.test(text)
        ? 'unmute'
        : /\b(dark|light)\s+mode\b/i.test(text)
          ? /\b(light|off|disable)\b/i.test(text)
            ? 'light_mode'
            : 'dark_mode'
          : /\b(open|check)\b/i.test(text) &&
              /\b(email|emails|inbox)\b/i.test(text)
            ? 'open_email'
            : /\b(mute|silence)\b/i.test(text)
              ? 'mute'
              : /\bhide\b/i.test(text) &&
                  /\b(apps|applications|other|others|everything|all)\b/i.test(
                    text,
                  )
                ? 'hide_others'
                : /\b(quit|close)\b/i.test(text) &&
                    /\bapps?\b/i.test(text) &&
                    /\b(unused|used|using|idle)\b/i.test(text)
                  ? 'quit_unused'
                  : /\bquit\b/i.test(text)
                    ? 'quit_app'
                    : /\bcalendar\b/i.test(text)
                      ? 'open_calendar'
                      : /\bempty\b/i.test(text) && /\b(trash|bin)\b/i.test(text)
                        ? 'empty_trash'
                        : /\btabs?\b/i.test(text)
                          ? 'close_tabs'
                          : /\bdocuments\b/i.test(text)
                            ? 'tidy_documents'
                            : /\b(desktop|screenshots?)\b/i.test(text)
                              ? 'tidy_desktop'
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

/** ADR 0130: the running apps a quit_unused run would quit (not used today; frontmost, finder and qrtz never). */
const UNUSED: CtrlConfirm['items'] = [
  { key: '0a1b2c3d4e5f#0', kind: 'app', label: 'slack', unsaved: null },
  { key: '5f4e3d2c1b0a#0', kind: 'app', label: 'textedit', unsaved: true },
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
  const finishChain = (
    r: { run: CtrlRun; recs: Rec[] },
    intent: Intent,
    text = '',
  ) => {
    const level = intent === 'set_volume' ? levelOf(text) : null;
    if (level !== null)
      decide(
        r,
        act('choose_target', [opt(`level.${level}`, `${level}%`, 0.96)]),
      );
    if (intent === 'volume_up' || intent === 'volume_down') {
      const n = stepOf(text);
      decide(r, act('choose_target', [opt(`step.${n}`, `by ${n}`, 0.96)]));
    }
    for (const d of REST[intent]) decide(r, d);
    const s = stateAt(r.recs, r.recs.length);
    push(r, 'plan.produced', {
      plan: {
        irreversible:
          intent === 'quit_app' ||
          intent === 'quit_unused' ||
          intent === 'trash_old' ||
          intent === 'empty_trash',
        steps: s.decisions.map((d) => ({
          node: d.node,
          option: d.chosen,
          label: d.chosenLabel,
        })),
      },
    });
    if (intent === 'tidy_documents') {
      // ADR 0125: held before any folder is read; the run ends and says why.
      push(r, 'exec.held', { held: 'folder-not-yet' });
      push(r, 'task.finished', { finished: 'asked-out' });
      return;
    }
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
    if (intent === 'quit_unused') {
      push(r, 'confirm.requested', {
        confirm: {
          gateId: `g${r.recs.filter((x) => x.confirm).length}`,
          action: "quit apps you haven't used today",
          app: null,
          move: 'quit them without bringing anything forward',
          items: UNUSED,
          complete: true,
          rules: [
            'choose_intent:intent.quit_unused',
            'choose_move:route.workspace.quit_unused',
          ],
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
    if (intent === 'tidy_desktop') {
      r.recs.find((x) => x.row.kind === 'action.requested')!.batch = {
        files: [
          'Screenshot 2026-10-12 at 09.14.02.png',
          'Screen Recording 2026-10-15 at 16.20.11.mov',
        ],
        moved: [],
      };
      return;
    }
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
    // ADR 0129: laya-coreml @ 7b8d7a2, e8 L512 bucket + tokenizer.json.
    sizeBytes: 483_924_446,
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
        finishChain(r, intent, t.text);
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
            error: null,
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
