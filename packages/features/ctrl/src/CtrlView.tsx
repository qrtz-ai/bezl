import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  CtrlTaskRow,
  EmptyState,
  Heading,
  Input,
  Page,
  PlainList,
  RunHistory,
  RunTimeline,
  SchedulePicker,
  SettingsSection,
  Stack,
  Text,
  useToast,
} from '@qrtz/ui';
import {
  CTRL_TEXT_MAX,
  type CtrlRun,
  type CtrlRunView,
  type CtrlTask,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import {
  describeSchedule,
  eventItem,
  runItem,
  stateSummary,
  toSchedule,
  when,
  type ScheduleForm,
} from './schedule';

/**
 * ctrl: type a task, pick when it runs, see its runs. Runs are derived from each run's event log (crates/qrtz-ctrl);
 * the scrubber shows the state after any step without executing anything. The local scheduler (slice 3, ADR 0105)
 * records due runs in the background and pushes `ctrl:changed`; the decision layers (slice 4) still execute nothing.
 */
export default function CtrlView() {
  const qrtz = useQrtz();
  const toast = useToast();
  const changed = useQrtzEvent('ctrl:changed');
  const [tasks, setTasks] = useState<CtrlTask[]>([]);
  const [text, setText] = useState('');
  const [form, setForm] = useState<ScheduleForm>({
    kind: 'daily',
    time: '09:00',
    weekday: 1,
  });
  const [selected, setSelected] = useState<string | null>(null);
  const [runs, setRuns] = useState<CtrlRun[]>([]);
  const [run, setRun] = useState<CtrlRunView | null>(null);

  const fail = useCallback((e: Error) => toast(e.message, 'error'), [toast]);
  const loadTasks = useCallback(
    () => void qrtz.invoke('ctrl:tasks').then(setTasks, fail),
    [qrtz, fail],
  );
  const loadRuns = useCallback(
    (taskId: string) =>
      void qrtz.invoke('ctrl:runs', { taskId }).then(setRuns, fail),
    [qrtz, fail],
  );
  const openRun = useCallback(
    (runId: string, at?: number) =>
      void qrtz
        .invoke('ctrl:run', at === undefined ? { runId } : { runId, at })
        .then(setRun, fail),
    [qrtz, fail],
  );

  useEffect(loadTasks, [loadTasks, changed]);
  useEffect(() => {
    setRun(null);
    if (selected) loadRuns(selected);
    else setRuns([]);
  }, [selected, loadRuns, changed]);

  const schedule = toSchedule(form, new Date());
  const canAdd = !!text.trim() && !!schedule;
  const add = () => {
    if (!canAdd || !schedule) return;
    void qrtz
      .invoke('ctrl:create-task', { text: text.trim(), schedule })
      .then((t) => {
        setText('');
        setSelected(t.id);
        loadTasks();
      }, fail);
  };
  const runNow = (id: string) =>
    void qrtz.invoke('ctrl:run-now', { id }).then((r) => {
      setSelected(id);
      loadTasks();
      loadRuns(id);
      openRun(r.id);
    }, fail);
  const togglePause = (t: CtrlTask) =>
    void qrtz
      .invoke('ctrl:set-paused', { id: t.id, paused: !t.paused })
      .then(loadTasks, fail);
  const remove = (id: string) =>
    void qrtz.invoke('ctrl:delete-task', { id }).then(() => {
      if (selected === id) setSelected(null);
      loadTasks();
    }, fail);

  const current = tasks.find((t) => t.id === selected) ?? null;
  return (
    <Page data-testid="ctrl">
      <Stack dir="row" align="center" justify="between">
        <Heading>ctrl</Heading>
        <Text variant="caption" muted>
          runs in the background, never takes over your screen
        </Text>
      </Stack>
      <Stack as="form" gap={2} onSubmit={add} data-testid="ctrl-new">
        <Input
          aria-label="task"
          placeholder="every week clean up my downloads folder"
          value={text}
          maxLength={CTRL_TEXT_MAX}
          onChange={(e) => setText(e.target.value)}
          data-testid="ctrl-task-input"
        />
        <Stack dir="row" gap={2} align="center" justify="between">
          <SchedulePicker
            kind={form.kind}
            time={form.time}
            weekday={form.weekday}
            onKind={(kind) => setForm({ ...form, kind })}
            onTime={(time) => setForm({ ...form, time })}
            onWeekday={(weekday) => setForm({ ...form, weekday })}
          />
          <Button type="submit" disabled={!canAdd} data-testid="ctrl-task-add">
            add task
          </Button>
        </Stack>
      </Stack>
      <SettingsSection title="tasks" data-testid="ctrl-tasks">
        {tasks.length ? (
          <PlainList label="tasks">
            {tasks.map((t) => (
              <CtrlTaskRow
                key={t.id}
                text={t.text}
                schedule={describeSchedule(t.schedule)}
                meta={
                  t.lastRunAt === null
                    ? 'never run'
                    : `last run ${when(t.lastRunAt)}`
                }
                paused={t.paused}
                selected={t.id === selected}
                onSelect={() => setSelected(t.id)}
                onRun={() => runNow(t.id)}
                onPause={() => togglePause(t)}
                onDelete={() => remove(t.id)}
              />
            ))}
          </PlainList>
        ) : (
          <EmptyState title="no tasks yet" icon="command" />
        )}
      </SettingsSection>
      {current ? (
        <SettingsSection title="runs" data-testid="ctrl-history">
          {runs.length ? (
            <RunHistory
              runs={runs.map(runItem)}
              selected={run?.run.id ?? null}
              onSelect={(id) => openRun(id)}
            />
          ) : (
            <EmptyState title="no runs yet" icon="clock" />
          )}
          {run ? (
            <RunTimeline
              events={run.events.map(eventItem)}
              at={run.at}
              onAt={(n) => openRun(run.run.id, n)}
              summary={stateSummary(run.state)}
            />
          ) : null}
        </SettingsSection>
      ) : null}
    </Page>
  );
}
