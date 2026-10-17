import { useCallback, useEffect, useState } from 'react';
import { Button, Input, Text, useTheme } from '@qrtz/ui';
import { clockLabel, type MeetingNote } from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';

/**
 * Granola-style notes. `meetingId` omitted = the meeting being recorded (notes are stamped with the current
 * recording offset by main). Enter adds a note; click a note to edit, empty it to delete.
 */
export function Notes({
  meetingId,
  compact,
}: {
  meetingId?: string;
  compact?: boolean;
}) {
  const qrtz = useQrtz();
  const t = useTheme();
  const changed = useQrtzEvent('memo:changed');
  const [notes, setNotes] = useState<MeetingNote[]>([]);
  const [draft, setDraft] = useState('');
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    qrtz
      .invoke('memo:list-notes', meetingId ? { meetingId } : {})
      .then(setNotes, () => {});
  }, [qrtz, meetingId]);
  useEffect(load, [load, changed]);

  const add = () => {
    const text = draft.trim();
    if (!text) return;
    setDraft('');
    qrtz
      .invoke('memo:add-note', { text, ...(meetingId ? { meetingId } : {}) })
      .then(() => {
        setError(null);
        load();
      })
      .catch((e: Error) => {
        setDraft(text);
        setError(e.message);
      });
  };
  const save = () => {
    if (!editing) return;
    const text = editing.text.trim();
    const op = text
      ? qrtz.invoke('memo:update-note', { id: editing.id, text })
      : qrtz.invoke('memo:delete-note', { id: editing.id });
    setEditing(null);
    void op.then(load, (e: Error) => setError(e.message));
  };

  return (
    <div data-testid="notes" style={{ display: 'grid', gap: 8 }}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
        style={{ display: 'flex', gap: 8 }}
      >
        <Input
          aria-label="Add a note"
          placeholder={
            meetingId ? 'Add a note…' : 'Jot a note (saved with the timestamp)…'
          }
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          data-testid="note-input"
          maxLength={4000}
        />
        <Button
          type="submit"
          size="sm"
          variant="secondary"
          data-testid="note-add"
        >
          Add
        </Button>
      </form>
      {error ? (
        <Text size="sm" style={{ color: t.color.danger }} role="alert">
          {error}
        </Text>
      ) : null}
      <ul
        aria-label="Notes"
        style={{
          listStyle: 'none',
          margin: 0,
          padding: 0,
          display: 'grid',
          gap: 4,
          ...(compact ? { maxHeight: 140, overflow: 'auto' } : {}),
        }}
      >
        {notes.map((n) => (
          <li
            key={n.id}
            data-testid="note"
            style={{ display: 'flex', gap: 8, alignItems: 'baseline' }}
          >
            <Text size="xs" muted mono data-testid="note-time">
              {clockLabel(n.atMs)}
            </Text>
            {editing?.id === n.id ? (
              <Input
                autoFocus
                aria-label="Edit note"
                value={editing.text}
                onChange={(e) => setEditing({ id: n.id, text: e.target.value })}
                onBlur={save}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') save();
                  if (e.key === 'Escape') setEditing(null);
                }}
                data-testid="note-edit"
              />
            ) : (
              <button
                type="button"
                onClick={() => setEditing({ id: n.id, text: n.text })}
                aria-label={`${n.text} (edit note)`}
                style={{
                  all: 'unset',
                  cursor: 'text',
                  flex: 1,
                  color: t.color.text,
                  fontSize: t.font.size.sm,
                }}
              >
                {n.text}
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
