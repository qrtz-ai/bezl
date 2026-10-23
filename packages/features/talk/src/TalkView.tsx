import { useEffect, useState, type ReactNode } from 'react';
import {
  Button,
  DictionaryTerm,
  EmptyState,
  Input,
  Page,
  PlainList,
  SettingsSection,
  SettingRow,
  Stack,
  TalkHistoryRow,
  TalkPill,
  TermList,
  Text,
  useToast,
  useUserError,
  ErrorCallout,
  SpeechRequired,
  ModuleSettingsModal,
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
} from '@qrtz/core';
import { entryMeta, parseTerm } from './format';
import TalkSettings from './TalkSettings';

const CONSENT_NOTE =
  'parakeet downloads once from hugging face, is checked against the fingerprint built into qrtz, and stays on this mac.';

/**
 * ADR 0174: echo ships in qrtz.app, so its view is always in the sidebar. It's switched on and off on the apps page
 * (packages:set-enabled), not here. ADR 0175: while echo is on and Parakeet is configured but not installed, the page
 * offers the download in place (SpeechRequired, with the consent note).
 */
function useEcho() {
  const qrtz = useQrtz();
  const changed = useQrtzEvent('config:changed');
  const [cfg, setCfg] = useState<Config | null>(null);
  useEffect(() => {
    if (changed) setCfg(loadConfig(changed));
  }, [changed]);
  useEffect(() => {
    void qrtz.invoke('config:get').then(
      (c) => setCfg(loadConfig(c)),
      () => {},
    );
  }, [qrtz]);
  const err = useUserError();
  const fail = err.fail('change echo');
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
    on: !!cfg && isModuleOn(cfg, 'echo'),
    loaded: !!cfg,
    error: (
      <ErrorCallout
        error={err.error}
        onDismiss={err.clear}
        data-testid="echo-error"
      />
    ),
    download: () =>
      void qrtz.invoke('speech:download', { extensions: ['echo'] }).catch(fail),
    cancel: () =>
      void qrtz.invoke('speech:pause', { extensions: ['echo'] }).catch(fail),
  };
}

export default function TalkView() {
  const echo = useEcho();
  if (echo.loaded && !echo.on)
    return (
      <Page data-testid="talk-off">
        <SettingRow description="echo is off: turn it on in apps to dictate with fn" />
        {echo.notice}
      </Page>
    );
  return (
    <TalkOn
      error={echo.error}
      notice={echo.notice}
      onDownload={echo.download}
      onCancel={echo.cancel}
    />
  );
}

function TalkOn({
  error: echoError,
  notice,
  onDownload,
  onCancel,
}: {
  error: ReactNode;
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
  const { error, fail: failed, clear } = useUserError();
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

  const saveDict = (next: DictionaryEntry[]) =>
    void qrtz
      .invoke('talk:set-dictionary', { entries: next })
      .then(setDict, failed('save the dictionary'));
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
    <Page data-testid="talk">
      {echoError}
      <ErrorCallout error={error} onDismiss={clear} data-testid="talk-error" />
      {optsModal}
      {notice}
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
