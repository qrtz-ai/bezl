import { useCallback, useEffect, useState } from 'react';
import {
  AskCard,
  ConfirmCard,
  ConfirmOutcome,
  DecisionModelCard,
  StandingApprovals,
  Button,
  CtrlTaskRow,
  DecisionList,
  EmptyState,
  Heading,
  Input,
  Page,
  PlainList,
  PlanSummary,
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
  type CtrlModelState,
  type CtrlTask,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import {
  decisionItem,
  describeApproval,
  describeSchedule,
  eventItem,
  nodeQuestion,
  runItem,
  stateSummary,
  toSchedule,
  when,
  type ScheduleForm,
} from './schedule';

/**
 * ctrl: type a task, pick when it runs, see its runs. Runs are derived from each run's event log (crates/qrtz-ctrl);
 * the scrubber shows the state after any step without executing anything. The local scheduler (slice 3, ADR 0105)
 * records due runs in the background and pushes `ctrl:changed`. The decision layers (slice 4, ADR 0106) decide each
 * run's plan or ask which option was meant; nothing executes yet (slice 5).
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
  const [model, setModel] = useState<CtrlModelState | null>(null);

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
  // ADR 0116: a backend without the model channels simply hides the section.
  useEffect(() => {
    void qrtz.invoke('ctrl:model').then(setModel, () => setModel(null));
  }, [qrtz, changed]);
  const consent = (yes: boolean) =>
    void qrtz
      .invoke('ctrl:model-consent', { consent: yes })
      .then(setModel, fail);
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
  const answer = (runId: string, decisionId: string, optionId: string | null) =>
    void qrtz
      .invoke('ctrl:answer', { runId, decisionId, optionId })
      .then((v) => {
        setRun(v);
        if (selected) loadRuns(selected);
      }, fail);
  const confirm = (
    runId: string,
    gateId: string,
    a: 'approve' | 'remember' | 'deny',
  ) =>
    void qrtz.invoke('ctrl:confirm', { runId, gateId, answer: a }).then((v) => {
      setRun(v);
      loadTasks();
      if (selected) loadRuns(selected);
    }, fail);
  const revoke = (taskId: string, approvalId: string) =>
    void qrtz
      .invoke('ctrl:revoke-approval', { taskId, approvalId })
      .then(loadTasks, fail);
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
      {model ? (
        <SettingsSection
          title="decision model"
          data-testid="ctrl-model-section"
        >
          <DecisionModelCard model={model} onConsent={consent} />
        </SettingsSection>
      ) : null}
      {current?.approvals?.length ? (
        <SettingsSection
          title="allowed without asking"
          data-testid="ctrl-standing"
        >
          <StandingApprovals
            approvals={current.approvals.map((a) => ({
              id: a.id,
              label: describeApproval(a),
            }))}
            onRevoke={(id) => revoke(current.id, id)}
          />
        </SettingsSection>
      ) : null}
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
          {run?.state.ask && run.at === run.events.length ? (
            <AskCard
              question={nodeQuestion(run.state.ask.node)}
              options={run.state.ask.options}
              onPick={(id) => answer(run.run.id, run.state.ask!.decisionId, id)}
              onCancel={() =>
                answer(run.run.id, run.state.ask!.decisionId, null)
              }
            />
          ) : null}
          {run?.state.confirm &&
          !run.state.confirm.answer &&
          run.at === run.events.length ? (
            <ConfirmCard
              action={run.state.confirm.action}
              app={run.state.confirm.app}
              items={run.state.confirm.items}
              complete={run.state.confirm.complete}
              onApprove={() =>
                confirm(run.run.id, run.state.confirm!.gateId, 'approve')
              }
              onRemember={() =>
                confirm(run.run.id, run.state.confirm!.gateId, 'remember')
              }
              onDeny={() =>
                confirm(run.run.id, run.state.confirm!.gateId, 'deny')
              }
            />
          ) : null}
          {run?.state.confirm?.answer ? (
            <ConfirmOutcome answer={run.state.confirm.answer} />
          ) : null}
          {run?.state.plan ? (
            <PlanSummary
              steps={run.state.plan.steps.map((s) => s.label)}
              irreversible={run.state.plan.irreversible}
            />
          ) : null}
          {run?.state.decisions.length ? (
            <DecisionList decisions={run.state.decisions.map(decisionItem)} />
          ) : null}
        </SettingsSection>
      ) : null}
    </Page>
  );
}
