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
} from '@qrtz/ui';
import type {
  DictionaryEntry,
  TalkEntry,
  TalkView as TalkState,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { entryMeta, parseTerm } from './format';

export default function TalkView() {
  const qrtz = useQrtz();
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

  return (
    <Page data-testid="talk">
      <Stack dir="row" align="center" justify="between">
        <Heading>talk</Heading>
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
            {listening ? 'Stop' : 'Dictate'}
          </Button>
        </Stack>
      </Stack>
      {state && state.phase !== 'idle' ? (
        <Stack data-testid="talk-live">
          <TalkPill
            inline
            phase={state.phase}
            level={state.level}
            text={state.text}
            partial={state.partial}
            {...(state.error ? { error: state.error } : {})}
          />
        </Stack>
      ) : null}
      <SettingsSection title="history" data-testid="talk-history">
        {history.length ? (
          <PlainList label="Dictation history">
            {history.map((e) => (
              <TalkHistoryRow
                key={e.id}
                text={e.text}
                meta={entryMeta(e)}
                {...(e.error ? { flag: e.error } : {})}
                onCopy={() =>
                  void qrtz
                    .invoke('talk:copy', { id: e.id })
                    .then(() => toast('Copied', 'success'))
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
            aria-label="Add a word"
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
            Add
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
