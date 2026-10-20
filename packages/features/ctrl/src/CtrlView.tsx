import { useCallback, useEffect, useState } from 'react';
import {
  AskCard,
  Button,
  ConfirmCard,
  DecisionList,
  DecisionModelCard,
  EmptyState,
  Input,
  ModuleSettingsButton,
  ModuleSettingsModal,
  Page,
  RunHistory,
  RunTimeline,
  SettingsSection,
  Stack,
  StopBar,
  Text,
  useToast,
} from '@qrtz/ui';
import {
  CTRL_TEXT_MAX,
  SETTINGS_PATH,
  type CtrlModelState,
  type CtrlRunSummary,
  type CtrlRunView,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent, useSubAppRoute } from '@qrtz/core/react';
import { decisionItem, eventItem, live, runItem, runSummary } from './run';
import CtrlSettings from './CtrlSettings';

/**
 * ctrl do (ADR 0214): type one command, ctrl does it on screen through a chain of small decisions over the
 * accessibility tree. Esc (or any key, click or scroll of yours) stops it. When it isn't sure it asks for one of
 * its top three guesses; before a send, a purchase, a deletion or a settings change it asks for your ok.
 */
export default function CtrlView() {
  // ADR 0146 M1.4: this module's own settings open here, not in the global settings.
  const [settingsOpen, setSettingsOpen] = useState(false);
  const route = useSubAppRoute();
  useEffect(() => {
    if (route.path === SETTINGS_PATH) setSettingsOpen(true);
  }, [route.path]);
  const closeSettings = () => {
    setSettingsOpen(false);
    if (route.path === SETTINGS_PATH) route.navigate('', { replace: true });
  };
  const qrtz = useQrtz();
  const toast = useToast();
  const changed = useQrtzEvent('ctrl:changed');
  const [text, setText] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [runs, setRuns] = useState<CtrlRunSummary[]>([]);
  const [run, setRun] = useState<CtrlRunView | null>(null);
  const [model, setModel] = useState<CtrlModelState | null>(null);

  const fail = useCallback((e: Error) => toast(e.message, 'error'), [toast]);
  const loadRuns = useCallback(
    () => void qrtz.invoke('ctrl:runs').then(setRuns, fail),
    [qrtz, fail],
  );
  const openRun = useCallback(
    (runId: string, at?: number) =>
      void qrtz
        .invoke('ctrl:run', at === undefined ? { runId } : { runId, at })
        .then(setRun, fail),
    [qrtz, fail],
  );
  useEffect(loadRuns, [loadRuns, changed]);
  // ADR 0116: a backend without the model channels simply hides the section.
  useEffect(() => {
    void qrtz.invoke('ctrl:model').then(setModel, () => setModel(null));
  }, [qrtz, changed]);
  const consent = (yes: boolean) =>
    void qrtz
      .invoke('ctrl:model-consent', { consent: yes })
      .then(setModel, fail);

  const settle = (v: CtrlRunView) => {
    setBusy(null);
    setRun(v);
    loadRuns();
  };
  const failed = (e: Error) => {
    setBusy(null);
    fail(e);
  };
  const go = () => {
    const t = text.trim();
    if (!t || busy) return;
    setBusy('pending');
    void qrtz.invoke('ctrl:do', { text: t }).then((v) => {
      setText('');
      settle(v);
    }, failed);
  };
  const answer = (
    runId: string,
    decisionId: string,
    optionId: string | null,
  ) => {
    setBusy(runId);
    void qrtz
      .invoke('ctrl:answer', { runId, decisionId, optionId })
      .then(settle, failed);
  };
  const confirm = (runId: string, gateId: string, a: 'approve' | 'deny') => {
    setBusy(runId);
    void qrtz
      .invoke('ctrl:confirm', { runId, gateId, answer: a })
      .then(settle, failed);
  };
  const stop = (runId: string) =>
    void qrtz.invoke('ctrl:stop', { runId }).then(() => {
      loadRuns();
      if (run?.runId === runId) openRun(runId);
    }, fail);

  const optsModal = (
    <ModuleSettingsModal
      open={settingsOpen}
      module="ctrl"
      onClose={closeSettings}
      data-testid="ctrl-settings"
    >
      <CtrlSettings />
    </ModuleSettingsModal>
  );
  const stoppable =
    busy ?? (run && run.status === 'running' ? run.runId : null);
  return (
    <Page
      data-testid="ctrl"
      title="ctrl"
      actions={
        <>
          <Text variant="caption" muted truncate>
            does it on your screen · esc stops it
          </Text>
          <ModuleSettingsButton
            module="ctrl"
            onClick={() => setSettingsOpen(true)}
            data-testid="ctrl-settings-open"
          />
        </>
      }
    >
      {optsModal}
      <Stack
        as="form"
        dir="row"
        gap={2}
        align="center"
        onSubmit={go}
        data-testid="ctrl-do"
      >
        <Input
          aria-label="do"
          placeholder="do…"
          value={text}
          maxLength={CTRL_TEXT_MAX}
          disabled={!!busy}
          onChange={(e) => setText(e.target.value)}
          data-testid="ctrl-do-input"
        />
        <Button
          type="submit"
          disabled={!text.trim() || !!busy}
          data-testid="ctrl-do-go"
        >
          do
        </Button>
      </Stack>
      {stoppable ? (
        <StopBar
          onStop={() => (stoppable === 'pending' ? undefined : stop(stoppable))}
        />
      ) : null}
      {run ? (
        <SettingsSection title={run.text} data-testid="ctrl-current">
          <RunTimeline
            events={run.events.map(eventItem)}
            at={run.at}
            onAt={(n) => openRun(run.runId, n)}
            summary={runSummary(run)}
          />
          {run.ask && live(run) ? (
            <AskCard
              question={run.ask.question}
              options={run.ask.options}
              onPick={(id) => answer(run.runId, run.ask!.decisionId, id)}
              onCancel={() => answer(run.runId, run.ask!.decisionId, null)}
            />
          ) : null}
          {run.confirm && live(run) ? (
            <ConfirmCard
              action={run.confirm.action}
              app={run.confirm.app}
              rules={run.confirm.rules}
              onApprove={() =>
                confirm(run.runId, run.confirm!.gateId, 'approve')
              }
              onDeny={() => confirm(run.runId, run.confirm!.gateId, 'deny')}
            />
          ) : null}
          {run.decisions.length ? (
            <DecisionList decisions={run.decisions.map(decisionItem)} />
          ) : null}
        </SettingsSection>
      ) : null}
      <SettingsSection title="history" data-testid="ctrl-history">
        {runs.length ? (
          <RunHistory
            runs={runs.map(runItem)}
            selected={run?.runId ?? null}
            onSelect={(id) => openRun(id)}
          />
        ) : (
          <EmptyState title="nothing done yet" icon="command" />
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
    </Page>
  );
}
