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
  SpeechNotice,
  ModuleSettingsButton,
  ModuleSettingsPage,
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
  requestNavigate,
  type Config,
  type SpeechSetupStatus,
} from '@qrtz/core';
import { entryMeta, parseTerm } from './format';
import TalkSettings from './TalkSettings';

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
  const busy = BUSY.has(liveState ?? parakeet?.download.state ?? 'idle');
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
  // The speech notice is hidden while this offer is up, so a failed download's reason is shown here instead.
  const failed = (live?.download.state ?? parakeet?.download.state) === 'error';
  const offer =
    on && parakeet && needsParakeet && !busy ? (
      <Callout
        {...(failed
          ? {
              tone: 'warning' as const,
              title: `parakeet couldn't be downloaded${parakeet?.download.error ? `: ${parakeet.download.error}` : ''}`,
            }
          : { title: "parakeet isn't downloaded yet" })}
        action={
          <Button
            size="sm"
            variant="primary"
            icon="download"
            onClick={() => void download().then(setParakeet, fail)}
            data-testid="echo-download"
          >
            download parakeet
          </Button>
        }
        data-testid="echo-offer"
      >
        {consent}
      </Callout>
    ) : null;
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
  // On the "on" page the error takes the offer's place (the offer needs the status it couldn't read).
  return {
    notice,
    on,
    loaded: !!cfg,
    control,
    dialog,
    offer: statusError ?? offer,
    statusError,
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
      offer={echo.offer}
      notice={echo.notice}
    />
  );
}

function TalkOn({
  switchControl,
  dialog,
  offer,
  notice,
}: {
  switchControl: ReactNode;
  dialog: ReactNode;
  offer: ReactNode;
  notice: ReactNode;
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
  const speechSetup = useSpeechSetup({ check: false });
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

  if (settingsOpen)
    return (
      <ModuleSettingsPage
        module="echo"
        onClose={closeSettings}
        data-testid="talk-settings"
      >
        <TalkSettings />
      </ModuleSettingsPage>
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
            variant={listening ? 'danger' : 'primary'}
            onClick={toggle}
            disabled={busy}
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
      {dialog}
      {notice}
      {offer}
      {/* ADR 0175: one notice at a time. While echo's own download offer is up, the generic speech notice (and
          its settings link) stays hidden; it comes back for the progress bar once a download starts. */}
      {offer ? null : (
        <SpeechNotice
          status={speechSetup.speech}
          onOpen={() => requestNavigate({ app: 'qrtz', item: 'echo' })}
        />
      )}
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
                onRemove={() => saveDict(dict.filter((x) => x.term !== d.term))}
              />
            ))}
          </TermList>
        ) : null}
      </SettingsSection>
    </Page>
  );
}
