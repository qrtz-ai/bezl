import { useCallback, useEffect, useState, type ReactNode } from 'react';
import {
  Button,
  Callout,
  DictionaryTerm,
  EmptyState,
  Input,
  Kbd,
  Page,
  PlainList,
  SettingsSection,
  Stack,
  TalkHistoryRow,
  TalkPill,
  TermList,
  Text,
  useToast,
  SpeechRequired,
  ModuleSettingsButton,
  ModuleSettingsModal,
  Switch,
  ConfirmDialog,
} from '@qrtz/ui';
import type {
  DictionaryEntry,
  TalkEntry,
  TalkView as TalkState,
} from '@qrtz/core';
import {
  useQrtz,
  useQrtzEvent,
  useSpeechSetup,
  useSubAppRoute,
} from '@qrtz/core/react';
import {
  SETTINGS_PATH,
  isModuleOn,
  isOnBundledFallback,
  loadConfig,
  type Config,
  type SpeechSetupStatus,
} from '@qrtz/core';
import { entryMeta, parseTerm } from './format';
import TalkSettings from './TalkSettings';

const CONSENT_NOTE =
  'parakeet downloads once from hugging face, is checked against the fingerprint built into qrtz, and stays on this mac.';

const mb = (b: number) => `${Math.max(1, Math.round(b / 1e6))} mb`;

/** A download is under way (or waiting to resume): nothing to offer, the speech notice shows its bar. */
const BUSY = new Set(['checking', 'downloading', 'paused', 'installing']);

/**
 * ADR 0174: echo ships in qrtz.app, so its view is always in the sidebar and holds its switch. Turning it on asks
 * once to download Parakeet (from Hugging Face, checked against the hashes pinned in qrtz) unless it is already
 * there; turning it off keeps the model. The host lets this view change only `modules.echo.enabled`.
 *
 * ADR 0175: a fresh config has echo on, so the "on" page offers the same consent in place ("download parakeet")
 * while Parakeet is configured, not installed and not downloading; and the switch waits for `speech:status` (N180),
 * so a click can't turn echo on before we know whether it needs the download.
 *
 * N183: if `speech:status` fails, the view says so ("couldn't check speech, retry") and the switch is enabled
 * anyway. N184: turning echo on with the state still unknown reads `speech:status` again first, and uses that
 * answer (an installed Parakeet turns echo on with no question and no download, so echo never goes online when
 * Parakeet is here); only if it fails again, or says configured and not installed, does it ask and download.
 */
function useEchoSwitch() {
  const qrtz = useQrtz();
  const toast = useToast();
  const changed = useQrtzEvent('config:changed');
  const [cfg, setCfg] = useState<Config | null>(null);
  const [parakeet, setParakeet] = useState<SpeechSetupStatus | null>(null);
  const [statusFailed, setStatusFailed] = useState(false);
  const [asking, setAsking] = useState(false);
  // The host streams the (global) speech status while a download runs; its state says whether one is under way.
  const live = useQrtzEvent('speech:status');
  useEffect(() => {
    if (changed) setCfg(loadConfig(changed));
  }, [changed]);
  const refresh = useCallback(() => {
    void qrtz.invoke('config:get').then(
      (c) => setCfg(loadConfig(c)),
      () => {},
    );
    void qrtz.invoke('speech:status', { extensions: ['echo'] }).then(
      (s) => {
        setParakeet(s);
        setStatusFailed(false);
      },
      () => setStatusFailed(true),
    );
  }, [qrtz]);
  useEffect(refresh, [refresh]);
  const liveState = live?.download.state;
  // A download finished (or failed): read echo's own status again so the offer reflects it.
  useEffect(() => {
    if (liveState && !BUSY.has(liveState)) refresh();
  }, [liveState, refresh]);
  const on = !!cfg && isModuleOn(cfg, 'echo');
  // Unknown (speech:status failed): assume the download is needed, so turning on asks first.
  const needsParakeet = parakeet
    ? !!parakeet.configured && !parakeet.installed
    : statusFailed;
  const fail = (e: Error) => toast(e.message, 'error');
  const download = () =>
    qrtz.invoke('speech:download', { extensions: ['echo'] });
  const setOn = (v: boolean) =>
    qrtz
      .invoke('config:set', { patch: { modules: { echo: { enabled: v } } } })
      .then((c) => setCfg(loadConfig(c)));
  const turnOn = (needs: boolean) => {
    setAsking(false);
    setOn(true)
      .then(() => (needs ? download() : undefined))
      .then(refresh, fail);
  };
  const size = parakeet?.latestBytes ? ` (${mb(parakeet.latestBytes)})` : '';
  const consent = `echo needs its speech model, parakeet${size}. it downloads once from hugging face, is checked against the fingerprint built into qrtz, and stays on this mac.`;
  const toggle = (v: boolean) => {
    if (!v) return void setOn(false).catch(fail);
    // N184: the status is unknown (it failed): ask again before assuming a download is needed.
    if (parakeet === null)
      return void qrtz.invoke('speech:status', { extensions: ['echo'] }).then(
        (s) => {
          setParakeet(s);
          setStatusFailed(false);
          if (s.configured && !s.installed) setAsking(true);
          else turnOn(false);
        },
        () => {
          setStatusFailed(true);
          setAsking(true);
        },
      );
    // Already downloaded (or nothing to download in this build): no question.
    if (!needsParakeet) return turnOn(false);
    setAsking(true);
  };
  const dialog = (
    <ConfirmDialog
      open={asking}
      title="turn on echo?"
      body={consent}
      confirmLabel="download and turn on"
      onCancel={() => setAsking(false)}
      onConfirm={() => turnOn(true)}
      data-testid="echo-consent"
    />
  );
  const control = (
    <Switch
      checked={on}
      onChange={toggle}
      label="echo"
      disabled={!cfg || (!parakeet && !statusFailed)}
      data-testid="echo-switch"
    />
  );
  const statusError =
    statusFailed && !parakeet ? (
      <Callout
        tone="warning"
        title="couldn't check speech, retry"
        action={
          <Button
            size="sm"
            variant="secondary"
            icon="refresh"
            onClick={refresh}
            data-testid="echo-status-retry"
          >
            retry
          </Button>
        }
        data-testid="echo-status-error"
      />
    ) : null;
  // The host reports echo is running the copy inside qrtz.app (its store version couldn't start). Stub: the
  // host doesn't write `modules.echo.bundledFallback` yet, so this stays hidden until the self-update work does.
  const notice =
    cfg && isOnBundledFallback(cfg, 'echo') ? (
      <Text variant="caption" muted data-testid="echo-bundled-notice">
        echo is using the copy built into qrtz: its update couldn't start
      </Text>
    ) : null;
  return {
    notice,
    on,
    loaded: !!cfg,
    control,
    dialog,
    statusError,
    download: () => void download().then(setParakeet, fail),
    cancel: () =>
      void qrtz
        .invoke('speech:pause', { extensions: ['echo'] })
        .then(setParakeet, fail),
  };
}

export default function TalkView() {
  const echo = useEchoSwitch();
  if (echo.loaded && !echo.on)
    return (
      <Page data-testid="talk-off" title="echo" actions={echo.control}>
        {echo.notice}
        {echo.statusError}
        <EmptyState title="echo is off: turn it on to dictate with fn" />
        {echo.dialog}
      </Page>
    );
  return (
    <TalkOn
      switchControl={echo.control}
      dialog={echo.dialog}
      statusError={echo.statusError}
      notice={echo.notice}
      onDownload={echo.download}
      onCancel={echo.cancel}
    />
  );
}

function TalkOn({
  switchControl,
  dialog,
  statusError,
  notice,
  onDownload,
  onCancel,
}: {
  switchControl: ReactNode;
  dialog: ReactNode;
  statusError: ReactNode;
  notice: ReactNode;
  onDownload: () => void;
  onCancel: () => void;
}) {
  // ADR 0146 M1.4: this module's own settings open here, not in the global settings.
  const [settingsOpen, setSettingsOpen] = useState(false);
  // ADR 0161: the module manager's "settings" opens this view at `settings`.
  const route = useSubAppRoute();
  useEffect(() => {
    if (route.path === SETTINGS_PATH) setSettingsOpen(true);
  }, [route.path]);
  const closeSettings = () => {
    setSettingsOpen(false);
    if (route.path === SETTINGS_PATH) route.navigate('', { replace: true });
  };
  const qrtz = useQrtz();
  const speechSetup = useSpeechSetup({ check: false, extensions: ['echo'] });
  const toast = useToast();
  const live = useQrtzEvent('talk:state');
  const changed = useQrtzEvent('talk:history-changed');
  const [initial, setInitial] = useState<TalkState | null>(null);
  const [history, setHistory] = useState<TalkEntry[]>([]);
  const [dict, setDict] = useState<DictionaryEntry[]>([]);
  const [draft, setDraft] = useState('');
  const state = live ?? initial;

  useEffect(() => {
    void qrtz.invoke('talk:state').then(setInitial, () => {});
    void qrtz.invoke('talk:dictionary').then(setDict, () => {});
  }, [qrtz]);
  useEffect(() => {
    void qrtz.invoke('talk:history').then(setHistory, () => {});
  }, [qrtz, changed]);

  // Nothing dictates without the speech model (SpeechRequired gates the body; this is the header's button).
  const sp = speechSetup.speech;
  const speechReady = !sp || (sp.configured && !!sp.installed);
  const listening = state?.phase === 'listening';
  const busy = !!state && !['idle', 'error', 'listening'].includes(state.phase);
  const toggle = useCallback(() => {
    void qrtz
      .invoke(listening ? 'talk:stop' : 'talk:start')
      .catch((e: Error) => toast(e.message, 'error'));
  }, [qrtz, listening, toast]);

  const saveDict = (next: DictionaryEntry[]) =>
    void qrtz
      .invoke('talk:set-dictionary', { entries: next })
      .then(setDict, (e: Error) => toast(e.message, 'error'));
  const addTerm = () => {
    const t = parseTerm(draft);
    if (!t) return;
    setDraft('');
    saveDict([...dict, t]);
  };

  const optsModal = (
    <ModuleSettingsModal
      open={settingsOpen}
      module="echo"
      onClose={closeSettings}
      data-testid="talk-settings"
    >
      <TalkSettings />
    </ModuleSettingsModal>
  );
  return (
    <Page
      data-testid="talk"
      title="echo"
      actions={
        <>
          <Text variant="caption" muted truncate>
            hold <Kbd>fn</Kbd> or <Kbd>⌘⇧Space</Kbd>
          </Text>
          <Button
            // The model banner owns the one primary until speech is ready.
            variant={
              listening ? 'danger' : speechReady ? 'primary' : 'secondary'
            }
            onClick={toggle}
            disabled={busy || !speechReady}
            data-testid="talk-toggle"
          >
            {listening ? 'stop' : 'dictate'}
          </Button>
          <ModuleSettingsButton
            module="echo"
            onClick={() => setSettingsOpen(true)}
            data-testid="talk-settings-open"
          />
          {switchControl}
        </>
      }
    >
      {optsModal}
      {dialog}
      {notice}
      {statusError}
      {/* ADR 0175: echo can't dictate without its speech model, so until it's here the page shows only the
          download (with the consent note) over the blurred page. */}
      <SpeechRequired
        app="echo"
        status={speechSetup.speech}
        note={CONSENT_NOTE}
        onDownload={onDownload}
        onCancel={onCancel}
        data-testid="echo-offer"
        testIds={{ download: 'echo-download', retry: 'echo-download' }}
      >
        {state && state.phase !== 'idle' ? (
          <Stack data-testid="talk-live">
            <TalkPill
              inline
              phase={state.phase}
              level={state.level}
              {...(state.error ? { error: state.error } : {})}
            />
          </Stack>
        ) : null}
        <SettingsSection title="history" data-testid="talk-history">
          {history.length ? (
            <PlainList label="dictation history">
              {history.map((e) => (
                <TalkHistoryRow
                  key={e.id}
                  text={e.text}
                  meta={entryMeta(e)}
                  {...(e.error ? { flag: e.error } : {})}
                  onCopy={() =>
                    void qrtz
                      .invoke('talk:copy', { id: e.id })
                      .then(() => toast('copied', 'success'))
                  }
                  onDelete={() =>
                    void qrtz.invoke('talk:delete-entry', { id: e.id })
                  }
                />
              ))}
            </PlainList>
          ) : (
            <EmptyState title="nothing dictated yet" />
          )}
        </SettingsSection>
        <SettingsSection title="dictionary" data-testid="talk-dictionary">
          <Stack as="form" dir="row" gap={2} onSubmit={addTerm}>
            <Input
              aria-label="add a word"
              placeholder="qrtz = cue arts"
              value={draft}
              maxLength={200}
              onChange={(e) => setDraft(e.target.value)}
              data-testid="talk-term-input"
            />
            <Button
              variant="secondary"
              type="submit"
              disabled={!parseTerm(draft)}
              data-testid="talk-term-add"
            >
              add
            </Button>
          </Stack>
          {dict.length ? (
            <TermList>
              {dict.map((d) => (
                <DictionaryTerm
                  key={d.term}
                  term={d.term}
                  aliases={d.aliases}
                  onRemove={() =>
                    saveDict(dict.filter((x) => x.term !== d.term))
                  }
                />
              ))}
            </TermList>
          ) : null}
        </SettingsSection>
      </SpeechRequired>
    </Page>
  );
}
