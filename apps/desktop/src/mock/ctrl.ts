// The mocked backend's ctrl (e2e and `dev:mock`): the same store semantics as crates/qrtz-ctrl `store.rs`, in
// memory. Ids come from one append counter (`task-0`, `run-1`), runs start as `task.started` + `intent.pending`,
// and `ctrl:run` folds the first `at` events. The real persistence is covered by `cargo test -p qrtz-ctrl`.
import {
  AppError,
  type CtrlRun,
  type CtrlRunView,
  type CtrlTask,
  type Handlers,
} from '@qrtz/core';

export function ctrlHandlers(now = () => Date.now()): Partial<Handlers> {
  let seq = 0;
  const tasks: CtrlTask[] = [];
  const runs = new Map<
    string,
    { run: CtrlRun; events: CtrlRunView['events'] }
  >();
  const task = (id: string) => {
    const t = tasks.find((x) => x.id === id);
    if (!t) throw new AppError('E_NOT_FOUND', 'that task or run is gone');
    return t;
  };
  const stateAt = (events: CtrlRunView['events'], at: number) => {
    const kinds = events.slice(0, at).map((e) => e.kind);
    const pending =
      kinds.includes('intent.pending') && !kinds.includes('task.finished');
    return {
      actions: [],
      violations: 0,
      inconclusive: 0,
      quarantined: [],
      status: pending ? ('pending-intent' as const) : ('running' as const),
    };
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
      const run: CtrlRun = {
        id: `run-${seq++}`,
        taskId: id,
        at,
        status: 'pending-intent',
        events: 2,
        violations: 0,
      };
      runs.set(run.id, {
        run,
        events: [
          { seq: 0, kind: 'task.started', at, actionId: null },
          { seq: 1, kind: 'intent.pending', at, actionId: null },
        ],
      });
      t.runs += 1;
      t.lastRunAt = at;
      if (t.schedule.kind === 'once') t.nextRunAt = null;
      return run;
    },
    'ctrl:runs': ({ taskId }) =>
      [...runs.values()]
        .filter((r) => r.run.taskId === task(taskId).id)
        .map((r) => r.run)
        .reverse(),
    'ctrl:run': ({ runId, at }) => {
      const r = runs.get(runId);
      if (!r) throw new AppError('E_NOT_FOUND', 'that task or run is gone');
      const n = Math.min(at ?? r.events.length, r.events.length);
      return {
        run: r.run,
        events: r.events,
        at: n,
        state: stateAt(r.events, n),
      };
    },
  };
}
