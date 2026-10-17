import { useEffect, useMemo, useState } from 'react';
import { Dialog, Kbd, ListItem, Text, useTheme, useToast } from '@qrtz/ui';
import { requestNavigate, type SessionView } from '@qrtz/core';
import { registry } from './subapps';
import { shellClient } from './shellClient';
import { filterCommands, type Command } from './filter';

type Client = ReturnType<typeof shellClient>;
const listMeetings = (c: Client, query: string) =>
  c.invoke('memo:list', { query });

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
  const t = useTheme();
  const toast = useToast();
  const [q, setQ] = useState('');
  const [i, setI] = useState(0);
  const [session, setSession] = useState<SessionView | null>(null);
  const client = useMemo(shellClient, []);
  useEffect(() => {
    if (!open) return;
    setQ('');
    setI(0);
    client.invoke('memo:session').then(setSession, () => setSession(null));
  }, [open, client]);

  const [meetings, setMeetings] = useState<
    Awaited<ReturnType<typeof listMeetings>>
  >([]);
  useEffect(() => {
    const query = q.trim();
    if (!open || query.length < 2) return setMeetings([]);
    const h = setTimeout(() => {
      client.invoke('memo:list', { query }).then(
        (r) => setMeetings(r.slice(0, 6)),
        () => setMeetings([]),
      );
    }, 120);
    return () => clearTimeout(h);
  }, [q, open, client]);

  const commands = useMemo<Command[]>(() => {
    const recording = session?.phase === 'recording';
    const rec: Command = recording
      ? {
          id: 'memo.stop',
          title: 'Stop recording',
          hint: '⌘⇧R',
          keywords: 'memo meeting end',
          run: () =>
            client.invoke('memo:stop').then(() => toast('Recording stopped')),
        }
      : {
          id: 'memo.start',
          title: 'Start recording',
          hint: '⌘⇧R',
          keywords: 'memo meeting record',
          run: () =>
            client.invoke('memo:start', {}).then(() => {
              toast('Recording started', 'success');
              onNavigate('memo');
            }),
        };
    return [
      rec,
      ...registry.list().map(({ manifest }) => ({
        id: `nav.${manifest.id}`,
        title: `Go to ${manifest.name}`,
        hint: manifest.icon,
        keywords: `open ${manifest.id}`,
        run: () => onNavigate(manifest.id),
      })),
    ];
  }, [session, client, onNavigate, toast]);

  const results = [
    ...filterCommands(commands, q),
    ...meetings.map(
      ({ meeting, hit }): Command => ({
        id: `meeting.${meeting.id}`,
        title: meeting.title,
        hint: hit?.snippet
          ? hit.snippet.replace(/[[\]]/g, '').slice(0, 48)
          : 'Meeting',
        run: () => requestNavigate({ app: 'memo', meetingId: meeting.id }),
      }),
    ),
  ];
  const pick = (c: Command | undefined) => {
    if (!c) return;
    onClose();
    Promise.resolve(c.run()).catch((e: Error) => toast(e.message, 'error'));
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      width={560}
      top="14vh"
      label="Command palette"
    >
      <div data-testid="palette">
        <input
          data-testid="palette-input"
          value={q}
          placeholder="Type a command…"
          onChange={(e) => {
            setQ(e.target.value);
            setI(0);
          }}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown') {
              e.preventDefault();
              setI((x) => Math.min(x + 1, results.length - 1));
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              setI((x) => Math.max(x - 1, 0));
            } else if (e.key === 'Enter') {
              e.preventDefault();
              pick(results[i]);
            }
          }}
          style={{
            width: '100%',
            height: 52,
            padding: `0 ${t.space[5]}`,
            border: 'none',
            borderBottom: `1px solid ${t.color.border}`,
            background: 'transparent',
            color: t.color.text,
            fontSize: t.font.size.lg,
            fontFamily: 'inherit',
            outline: 'none',
          }}
        />
        <div
          role="listbox"
          style={{ padding: t.space[2], maxHeight: 340, overflow: 'auto' }}
        >
          {results.length === 0 ? (
            <div style={{ padding: t.space[4] }}>
              <Text muted size="sm">
                No matching commands
              </Text>
            </div>
          ) : (
            results.map((c, idx) => (
              <ListItem
                key={c.id}
                active={idx === i}
                onClick={() => pick(c)}
                data-testid={`cmd-${c.id}`}
                right={
                  c.hint ? (
                    <Text size="xs" mono muted>
                      {c.hint}
                    </Text>
                  ) : undefined
                }
              >
                <Text size="sm">{c.title}</Text>
              </ListItem>
            ))
          )}
        </div>
        <div
          style={{
            display: 'flex',
            gap: t.space[4],
            padding: `${t.space[2]} ${t.space[4]}`,
            borderTop: `1px solid ${t.color.border}`,
          }}
        >
          <Text size="xs" muted>
            <Kbd>↑</Kbd> <Kbd>↓</Kbd> navigate
          </Text>
          <Text size="xs" muted>
            <Kbd>↵</Kbd> run
          </Text>
          <Text size="xs" muted>
            <Kbd>esc</Kbd> close
          </Text>
        </div>
      </div>
    </Dialog>
  );
}
