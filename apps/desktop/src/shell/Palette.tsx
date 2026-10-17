import { useEffect, useMemo, useState } from 'react';
import { CommandPalette, isIconName, useToast } from '@qrtz/ui';
import { requestNavigate, type SessionView } from '@qrtz/core';
import { registry } from './subapps';
import { shellClient } from './shellClient';
import { filterCommands, type Command } from './filter';

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
    if (!open || query.length < 2) return setMeetings([]);
    const h = setTimeout(() => {
      client.invoke('note:list', { query }).then(
        (r) => setMeetings(r.slice(0, 6)),
        () => setMeetings([]),
      );
    }, 120);
    return () => clearTimeout(h);
  }, [q, open, client]);

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
      rec,
      ...registry.list().map(({ manifest }) => ({
        id: `nav.${manifest.id}`,
        title: `go to ${manifest.name}`,
        icon: manifest.icon,
        keywords: `open ${manifest.id}`,
        run: () => onNavigate(manifest.id),
      })),
    ];
  }, [session, client, onNavigate, toast]);

  const results = [
    ...filterCommands(commands, q),
    ...meetings.map(({ meeting, hit }): Command => ({
      id: `meeting.${meeting.id}`,
      title: meeting.title,
      hint: hit?.snippet
        ? hit.snippet.replace(/[[\]]/g, '').slice(0, 48)
        : 'meeting',
      icon: 'mic',
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
