import { useEffect, useMemo, useState } from 'react';
import {
  CommandPalette,
  ErrorCallout,
  isIconName,
  useToast,
  useUserError,
} from '@qrtz/ui';
import { isViewOn, requestNavigate, type SessionView } from '@qrtz/core';
import { registry } from './subapps';
import { shellClient } from './shellClient';
import { filterCommands, type Command } from './filter';
import { useModuleSwitches } from './useModuleSwitches';
import { QRTZ, QRTZ_PARTS, isReservedNav } from './routes';

const QRTZ_PAGE_PARTS = new Set(Object.values(QRTZ_PARTS));
/** Dev-only sub apps with a route but no palette entry (the gallery). */
const HIDDEN_FROM_NAV = new Set(['gallery']);

/**
 * The modules that get a "go to" entry: switched on, not a qrtz page part or dev-only, and (security review) not
 * posing as the shell's apps or opts entries by id or display name, any case or padding.
 */
export function paletteNavModules<M extends { id: string; name: string }>(
  manifests: readonly M[],
  isOn: (id: string) => boolean,
): M[] {
  return manifests.filter(
    (m) =>
      isOn(m.id) &&
      !QRTZ_PAGE_PARTS.has(m.id) &&
      !HIDDEN_FROM_NAV.has(m.id) &&
      !isReservedNav(m.id) &&
      !isReservedNav(m.name),
  );
}

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
  const { error, fail: failed, clear } = useUserError();
  const [q, setQ] = useState('');
  const [i, setI] = useState(0);
  const [session, setSession] = useState<SessionView | null>(null);
  const client = useMemo(shellClient, []);
  // ADR 0146: no commands or meetings for an app switched off on the qrtz page.
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
      // The qrtz page's two halves, apps (the app manager) and opts; the rest are the apps the sidebar lists.
      {
        id: 'nav.qrtz',
        title: 'go to apps',
        keywords: 'open apps install update remove manager store marketplace',
        run: () => onNavigate(QRTZ),
      },
      {
        id: 'nav.qrtz-settings',
        title: 'go to opts',
        keywords: 'open opts preferences settings theme backup logs updates',
        run: () => requestNavigate({ app: QRTZ, settings: true }),
      },
      ...paletteNavModules(
        registry.list().map(({ manifest }) => manifest),
        (id) => isViewOn(switches, id),
      ).map((manifest) => ({
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
      icon: 'edit-3',
      run: () => requestNavigate({ app: 'note', meetingId: meeting.id }),
    })),
  ];
  const pick = (c: Command | undefined) => {
    if (!c) return;
    clear();
    // Close on success; on failure the palette stays open and says what happened, inline.
    Promise.resolve(c.run()).then(onClose, failed(`run "${c.title}"`));
  };

  return (
    <CommandPalette
      open={open}
      onClose={() => {
        clear();
        onClose();
      }}
      notice={<ErrorCallout error={error} data-testid="palette-error" />}
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
