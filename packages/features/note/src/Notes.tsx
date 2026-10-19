import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ConfirmDialog, NotesApp } from '@qrtz/ui';
import { clockLabel, type MeetingNote } from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { fmtDate } from './format';

/** Autosave waits this long after the last keystroke (ADR 0218). Local SQLite, so no spinner is needed. */
export const SAVE_DEBOUNCE_MS = 400;

/** The list title: the first non-empty line, cut to a list-friendly length. */
export function noteTitle(text: string): string {
  const line = text
    .split('\n')
    .map((l) => l.trim())
    .find(Boolean);
  if (!line) return 'untitled';
  return line.length > 60 ? `${line.slice(0, 59)}…` : line;
}

/** Newest first, then filtered by a case-insensitive substring. Pure, so it is unit tested. */
export function visibleNotes(notes: MeetingNote[], query: string) {
  const q = query.trim().toLowerCase();
  return [...notes]
    .sort((a, b) => b.createdAt - a.createdAt)
    .filter((n) => !q || n.text.toLowerCase().includes(q));
}

/**
 * A meeting's notes as a small notes app (ADR 0218): list + search on the left, one full-height editor on the
 * right that autosaves. `meetingId` omitted = the meeting being recorded (a new note is stamped with the
 * current recording offset by main). Same channels as before: note:list-notes / add-note / update-note /
 * delete-note, so storage, capabilities and the declared reads are unchanged.
 */
export function Notes({
  meetingId,
  compact,
}: {
  meetingId?: string;
  compact?: boolean;
}) {
  const qrtz = useQrtz();
  const changed = useQrtzEvent('note:changed');
  const [notes, setNotes] = useState<MeetingNote[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [status, setStatus] = useState<'saved' | 'saving' | 'idle'>('idle');
  const [confirm, setConfirm] = useState(false);

  // Saves run one at a time so a fast typist can never create the same new note twice.
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const idRef = useRef<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<string | null>(null);
  idRef.current = selected;

  const load = useCallback(() => {
    qrtz
      .invoke('note:list-notes', meetingId ? { meetingId } : {})
      .then(setNotes, () => {});
  }, [qrtz, meetingId]);
  useEffect(load, [load, changed]);

  const persist = useCallback(
    (text: string) => {
      const run = async () => {
        const body = text.trim();
        // An empty note is never written; delete is explicit.
        if (!body) return;
        const id = idRef.current;
        if (id === 'new') {
          const n = await qrtz.invoke('note:add-note', {
            text: body,
            ...(meetingId ? { meetingId } : {}),
          });
          if (idRef.current === 'new') {
            idRef.current = n.id;
            setSelected(n.id);
          }
        } else if (id) {
          await qrtz.invoke('note:update-note', { id, text: body });
        }
      };
      setStatus('saving');
      queue.current = queue.current.then(run).then(
        () => {
          setError(null);
          setStatus('saved');
          load();
        },
        (e: Error) => setError(e.message),
      );
      return queue.current;
    },
    [qrtz, meetingId, load],
  );

  const flush = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const text = pending.current;
    pending.current = null;
    if (text !== null) void persist(text);
  }, [persist]);
  // Leaving the tab or the meeting never drops the last keystrokes.
  useEffect(() => flush, [flush]);

  const onDraft = (text: string) => {
    setDraft(text);
    pending.current = text;
    setStatus('idle');
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(flush, SAVE_DEBOUNCE_MS);
  };
  const open = (id: string) => {
    flush();
    // The queued save still targets the old note: wait for it before switching.
    void queue.current.then(() => {
      setSelected(id);
      idRef.current = id;
      setDraft(notes.find((n) => n.id === id)?.text ?? '');
      setStatus('idle');
    });
  };
  const create = () => {
    flush();
    void queue.current.then(() => {
      setSelected('new');
      idRef.current = 'new';
      setDraft('');
      setStatus('idle');
    });
  };
  const remove = () => {
    setConfirm(false);
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    pending.current = null;
    void queue.current.then(() => {
      const id = idRef.current;
      setSelected(null);
      idRef.current = null;
      setDraft('');
      if (!id || id === 'new') return;
      qrtz
        .invoke('note:delete-note', { id })
        .then(load, (e: Error) => setError(e.message));
    });
  };

  const items = useMemo(
    () =>
      visibleNotes(notes, query).map((n) => ({
        id: n.id,
        title: noteTitle(n.text),
        meta: meetingId
          ? `${clockLabel(n.atMs)} · ${fmtDate(n.createdAt)}`
          : clockLabel(n.atMs),
      })),
    [notes, query, meetingId],
  );

  return (
    <>
      <NotesApp
        notes={items}
        selectedId={selected}
        draft={draft}
        query={query}
        onQuery={setQuery}
        onSelect={open}
        onNew={create}
        onDraft={onDraft}
        onDelete={() =>
          selected === 'new' && !draft.trim() ? remove() : setConfirm(true)
        }
        status={
          status === 'saving' ? 'saving…' : status === 'saved' ? 'saved' : ''
        }
        error={error}
        emptyTitle={notes.length ? 'pick a note' : 'no notes yet'}
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
