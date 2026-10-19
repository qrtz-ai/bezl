import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  DictionaryTerm,
  EmptyState,
  Heading,
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
import { SETTINGS_PATH, requestNavigate } from '@qrtz/core';
import { entryMeta, parseTerm } from './format';
import TalkSettings from './TalkSettings';

export default function TalkView() {
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
    <Page data-testid="talk">
      <Stack dir="row" align="center" justify="between">
        <Heading>echo</Heading>
        <Stack dir="row" gap={2} align="center">
          <Text variant="caption" muted>
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
        </Stack>
      </Stack>
      <SpeechNotice
        status={speechSetup.speech}
        onOpenSettings={() => requestNavigate({ app: 'settings' })}
      />
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
