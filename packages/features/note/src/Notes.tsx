import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Button,
  Chip,
  ConfirmDialog,
  NotesApp,
  Select,
  Stack,
  Text,
  userError,
  type UserError,
} from '@qrtz/ui';
import { AppError, type Doc, type DocSummary } from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { fmtDate } from './format';

/** Autosave waits this long after the last keystroke (ADR 0219). */
export const SAVE_DEBOUNCE_MS = 400;

/** The list title: the first non-empty line (a `#` marker dropped). Mirrors qrtz_storage::vault::title_of. */
export function noteTitle(text: string): string {
  const line = text
    .split('\n')
    .map((l) => l.trim().replace(/^#+/, '').trim())
    .find(Boolean);
  if (!line) return 'untitled';
  return line.length > 60 ? `${line.slice(0, 59)}…` : line;
}

const isConflict = (e: unknown) =>
  e instanceof AppError &&
  (e.options.details as { conflict?: boolean } | undefined)?.conflict === true;

/**
 * nano's notes (ADR 0220): `.md` files in the notes folder, through `doc:*`. Without `meetingId` every note is
 * listed and a new note is standalone; with it, only that meeting's notes, and a new one is linked to it.
 * Autosave carries the hash the editor loaded, so an edit made outside qrtz is never overwritten silently: the
 * note reloads when it has no unsaved typing, and otherwise asks "keep mine" (the other version is kept as a
 * conflict copy) or "use theirs".
 */
export function Notes({
  meetingId,
  compact,
  linkable,
}: {
  meetingId?: string;
  compact?: boolean;
  /** Show 'link meeting' on the open note (the standalone notes page). */
  linkable?: boolean;
}) {
  const qrtz = useQrtz();
  const changed = useQrtzEvent('doc:changed');
  const [notes, setNotes] = useState<DocSummary[]>([]);
  const [error, setError] = useState<UserError | null>(null);
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState<Doc | null>(null);
  const [isNew, setIsNew] = useState(false);
  const [draft, setDraft] = useState('');
  const [status, setStatus] = useState<'saved' | 'saving' | 'idle'>('idle');
  const [confirm, setConfirm] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [meetings, setMeetings] = useState<{ id: string; title: string }[]>([]);

  // Saves run one at a time; `cur` is the note being edited (id + the hash its text is based on).
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const cur = useRef<{ id: string; hash: string } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<string | null>(null);

  const load = useCallback(() => {
    const run = () =>
      qrtz
        .invoke('doc:list', {
          ...(query.trim() ? { query: query.trim() } : {}),
          ...(meetingId ? { meetingId } : {}),
        })
        .then(setNotes, () => {});
    if (!query) {
      void run();
      return;
    }
    const t = setTimeout(run, 150);
    return () => clearTimeout(t);
  }, [qrtz, meetingId, query]);
  useEffect(load, [load, changed]);

  const show = useCallback((d: Doc) => {
    cur.current = { id: d.id, hash: d.hash };
    setOpen(d);
    setDraft(d.body);
    setIsNew(false);
    setConflict(false);
  }, []);

  const persist = useCallback(
    (text: string, force = false) => {
      const run = async () => {
        let c = cur.current;
        if (!c && text.trim()) {
          // A new note is written on its first real keystrokes, never as an empty file.
          const d = await qrtz.invoke(
            'doc:create',
            meetingId ? { meetingId } : {},
          );
          cur.current = c = { id: d.id, hash: d.hash };
        }
        if (!c) return;
        const d = await qrtz.invoke('doc:save', {
          id: c.id,
          body: text,
          baseHash: c.hash,
          ...(force ? { force } : {}),
        });
        cur.current = { id: d.id, hash: d.hash };
        setOpen(d);
        setIsNew(false);
      };
      setStatus('saving');
      queue.current = queue.current.then(run).then(
        () => {
          setError(null);
          setConflict(false);
          setStatus('saved');
        },
        (e: unknown) => {
          setStatus('idle');
          if (isConflict(e)) {
            pending.current = text;
            setConflict(true);
          } else setError(userError(e, 'save this note'));
        },
      );
      return queue.current;
    },
    [qrtz, meetingId],
  );

  const flush = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const text = pending.current;
    pending.current = null;
    if (text !== null) void persist(text);
  }, [persist]);
  useEffect(() => flush, [flush]);

  // Changed on disk (another editor, the HUD, a sync): reload the open note unless the user is mid-typing.
  useEffect(() => {
    const c = cur.current;
    if (!c || !changed?.ids.includes(c.id) || pending.current !== null) return;
    void queue.current.then(() =>
      qrtz.invoke('doc:get', { id: c.id }).then(
        (d) => {
          if (pending.current === null && d.hash !== cur.current?.hash) show(d);
        },
        () => {},
      ),
    );
  }, [changed, qrtz, show]);

  const onDraft = (text: string) => {
    setDraft(text);
    pending.current = text;
    setStatus('idle');
    if (conflict) return; // wait for the user's choice
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(flush, SAVE_DEBOUNCE_MS);
  };
  const select = (id: string) => {
    flush();
    void queue.current.then(() =>
      qrtz
        .invoke('doc:get', { id })
        .then(show, (e: unknown) => setError(userError(e, 'open this note'))),
    );
  };
  const create = () => {
    flush();
    void queue.current.then(() => {
      cur.current = null;
      setOpen(null);
      setIsNew(true);
      setDraft('');
      setStatus('idle');
      setConflict(false);
    });
  };
  const close = () => {
    cur.current = null;
    setOpen(null);
    setIsNew(false);
    setDraft('');
  };
  const remove = () => {
    setConfirm(false);
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    pending.current = null;
    void queue.current.then(() => {
      const c = cur.current;
      close();
      if (!c) return;
      qrtz
        .invoke('doc:trash', { id: c.id })
        .catch((e: unknown) =>
          setError(userError(e, 'move this note to the trash')),
        );
    });
  };
  const keepMine = () => {
    const text = pending.current ?? draft;
    pending.current = null;
    void persist(text, true);
  };
  const useTheirs = () => {
    pending.current = null;
    const c = cur.current;
    if (c) void qrtz.invoke('doc:get', { id: c.id }).then(show);
  };

  useEffect(() => {
    if (!linkable) return;
    qrtz.invoke('note:list', { limit: 30 }).then(
      (r) =>
        setMeetings(
          r.map((i) => ({ id: i.meeting.id, title: i.meeting.title })),
        ),
      () => {},
    );
  }, [qrtz, linkable]);
  const link = (mid: string, on: boolean) => {
    const c = cur.current;
    if (!c) return;
    flush();
    void queue.current.then(() =>
      qrtz
        .invoke(on ? 'doc:link-meeting' : 'doc:unlink-meeting', {
          id: c.id,
          meetingId: mid,
        })
        .then(show, (e: unknown) =>
          setError(
            userError(e, on ? 'link the meeting' : 'unlink the meeting'),
          ),
        ),
    );
  };

  const items = useMemo(
    () =>
      notes.map((n) => ({
        id: n.id,
        title: n.title,
        meta: fmtDate(n.updatedAt),
      })),
    [notes],
  );
  const selectedId = isNew ? 'new' : (open?.id ?? null);
  const title = (id: string) =>
    meetings.find((m) => m.id === id)?.title ?? 'meeting';

  return (
    <>
      <NotesApp
        notes={items}
        selectedId={selectedId}
        draft={draft}
        query={query}
        onQuery={setQuery}
        onSelect={select}
        onNew={create}
        onDraft={onDraft}
        onDelete={() => (cur.current ? setConfirm(true) : close())}
        status={
          status === 'saving' ? 'saving…' : status === 'saved' ? 'saved' : ''
        }
        error={error}
        emptyTitle={notes.length ? 'pick a note' : 'no notes yet'}
        readOnly={open?.readOnly ?? false}
        banner={
          open?.readOnly ? (
            <Text variant="caption" data-testid="note-read-only">
              read-only: this file isn't plain text qrtz can safely edit, so it
              is never changed
            </Text>
          ) : conflict ? (
            <Stack dir="row" gap={2} align="center" data-testid="note-conflict">
              <Text variant="caption">changed outside qrtz</Text>
              <Button size="sm" variant="secondary" onClick={keepMine}>
                keep mine
              </Button>
              <Button size="sm" variant="ghost" onClick={useTheirs}>
                use theirs
              </Button>
            </Stack>
          ) : undefined
        }
        tools={
          linkable && open ? (
            <>
              {open.meetings.map((m) => (
                <Chip
                  key={m}
                  onClick={() => link(m, false)}
                  aria-label={`unlink ${title(m)}`}
                  data-testid="note-meeting"
                >
                  {title(m)}
                </Chip>
              ))}
              <Select
                aria-label="link meeting"
                data-testid="note-link-meeting"
                value=""
                onChange={(e) => e.target.value && link(e.target.value, true)}
              >
                <option value="">link meeting</option>
                {meetings
                  .filter((m) => !open.meetings.includes(m.id))
                  .map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.title}
                    </option>
                  ))}
              </Select>
            </>
          ) : undefined
        }
        {...(compact ? { compact } : {})}
      />
      <ConfirmDialog
        open={confirm}
        title="delete this note?"
        body={noteTitle(draft)}
        confirmLabel="delete"
        data-testid="confirm-delete-note"
        onCancel={() => setConfirm(false)}
        onConfirm={remove}
      />
    </>
  );
}
