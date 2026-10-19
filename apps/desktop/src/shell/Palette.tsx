import { useEffect, useMemo, useState } from 'react';
import { CommandPalette, isIconName, useToast } from '@qrtz/ui';
import { isViewOn, requestNavigate, type SessionView } from '@qrtz/core';
import { registry } from './subapps';
import { shellClient } from './shellClient';
import { filterCommands, type Command } from './filter';
import { useModuleSwitches } from './useModuleSwitches';

type Client = ReturnType<typeof shellClient>;
const listMeetings = (c: Client, query: string) =>
  c.invoke('note:list', { query });

/** ⌘K command palette: navigate sub apps, start/stop recording. */
export function Palette({
  open,
  onClose,
  onNavigate,
}: {
  open: boolean;
  onClose: () => void;
  onNavigate: (id: string) => void;
}) {
  const toast = useToast();
  const [q, setQ] = useState('');
  const [i, setI] = useState(0);
  const [session, setSession] = useState<SessionView | null>(null);
  const client = useMemo(shellClient, []);
  // ADR 0146: no commands or meetings for a module switched off in settings → modules.
  const switches = useModuleSwitches();
  const nanoOn = isViewOn(switches, 'note');
  useEffect(() => {
    if (!open) return;
    setQ('');
    setI(0);
    client.invoke('note:session').then(setSession, () => setSession(null));
  }, [open, client]);

  const [meetings, setMeetings] = useState<
    Awaited<ReturnType<typeof listMeetings>>
  >([]);
  useEffect(() => {
    const query = q.trim();
    if (!open || !nanoOn || query.length < 2) return setMeetings([]);
    const h = setTimeout(() => {
      client.invoke('note:list', { query }).then(
        (r) => setMeetings(r.slice(0, 6)),
        () => setMeetings([]),
      );
    }, 120);
    return () => clearTimeout(h);
  }, [q, open, client, nanoOn]);

  const commands = useMemo<Command[]>(() => {
    const recording = ['starting', 'recording', 'reconnecting'].includes(
      session?.phase ?? '',
    );
    const rec: Command = recording
      ? {
          id: 'note.stop',
          title: 'stop recording',
          hint: '⌘⇧R',
          icon: 'stop',
          keywords: 'note meeting end',
          run: () =>
            client.invoke('note:stop').then(() => toast('recording stopped')),
        }
      : {
          id: 'note.start',
          title: 'start recording',
          hint: '⌘⇧R',
          icon: 'circle',
          keywords: 'note meeting record',
          run: () =>
            client.invoke('note:start', {}).then(() => {
              toast('recording started', 'success');
              onNavigate('note');
            }),
        };
    return [
      ...(nanoOn ? [rec] : []),
      ...registry
        .list()
        .filter(({ manifest }) => isViewOn(switches, manifest.id))
        .map(({ manifest }) => ({
          id: `nav.${manifest.id}`,
          title: `go to ${manifest.name}`,
          icon: manifest.icon,
          keywords: `open ${manifest.id}`,
          run: () => onNavigate(manifest.id),
        })),
    ];
  }, [session, client, onNavigate, toast, nanoOn, switches]);

  const results = [
    ...filterCommands(commands, q),
    ...meetings.map(({ meeting, hit }): Command => ({
      id: `meeting.${meeting.id}`,
      title: meeting.title,
      hint: hit?.snippet
        ? hit.snippet.replace(/[[\]]/g, '').slice(0, 48)
        : 'meeting',
      icon: 'notebook',
      run: () => requestNavigate({ app: 'note', meetingId: meeting.id }),
    })),
  ];
  const pick = (c: Command | undefined) => {
    if (!c) return;
    onClose();
    Promise.resolve(c.run()).catch((e: Error) => toast(e.message, 'error'));
  };

  return (
    <CommandPalette
      open={open}
      onClose={onClose}
      query={q}
      onQuery={(v) => {
        setQ(v);
        setI(0);
      }}
      results={results.map((c) => ({
        id: c.id,
        title: c.title,
        ...(c.hint ? { hint: c.hint } : {}),
        ...(isIconName(c.icon) ? { icon: c.icon } : {}),
        testId: `cmd-${c.id}`,
      }))}
      index={i}
      onIndex={setI}
      onPick={(idx) => pick(results[idx])}
    />
  );
}
