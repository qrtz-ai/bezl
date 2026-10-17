import { useCallback, useEffect, useState } from 'react';
import { NotesEditor } from '@qrtz/ui';
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
  const changed = useQrtzEvent('memo:changed');
  const [notes, setNotes] = useState<MeetingNote[]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    qrtz
      .invoke('memo:list-notes', meetingId ? { meetingId } : {})
      .then(setNotes, () => {});
  }, [qrtz, meetingId]);
  useEffect(load, [load, changed]);

  const add = (text: string) => {
    qrtz
      .invoke('memo:add-note', { text, ...(meetingId ? { meetingId } : {}) })
      .then(() => {
        setError(null);
        load();
      })
      .catch((e: Error) => setError(e.message));
  };
  const update = (id: string, text: string) => {
    const op = text
      ? qrtz.invoke('memo:update-note', { id, text })
      : qrtz.invoke('memo:delete-note', { id });
    void op.then(load, (e: Error) => setError(e.message));
  };

  return (
    <NotesEditor
      notes={notes.map((n) => ({
        id: n.id,
        time: clockLabel(n.atMs),
        text: n.text,
      }))}
      onAdd={add}
      onUpdate={update}
      error={error}
      placeholder="Note"
      {...(compact ? { compact } : {})}
    />
  );
}
