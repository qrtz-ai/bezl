import { useEffect, useRef, type ReactNode } from 'react';
import { useTheme } from './theme';
import { Text } from './Text';
import { Input } from './Input';
import { EmptyState } from './EmptyState';
import { IconButton, ListRow } from './Layout';
import { typeStyle } from './tokens';

export interface NotesAppItem {
  id: string;
  /** First non-empty line of the note. */
  title: string;
  /** Short date/time label for the list. */
  meta: string;
}

/**
 * A notes app (ADR 0218): a searchable list of notes on the left, one full-height plain-text editor on the
 * right. Pure view: the caller owns the notes, the draft, saving and search. Square, bordered, monospace,
 * like the rest of qrtz; no shadows, no animation.
 */
export function NotesApp({
  notes,
  selectedId,
  draft,
  query,
  compact,
  status,
  error,
  onQuery,
  onSelect,
  onNew,
  onDraft,
  onDelete,
  editorLabel = 'note',
  emptyTitle = 'no notes yet',
}: {
  notes: NotesAppItem[];
  /** Selected note id, `'new'` for an unsaved new note, or null when nothing is open. */
  selectedId: string | null;
  draft: string;
  query: string;
  compact?: boolean;
  /** Save state shown under the editor ("saved", "saving…"). */
  status?: ReactNode;
  error?: string | null;
  onQuery: (q: string) => void;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDraft: (text: string) => void;
  onDelete: () => void;
  editorLabel?: string;
  emptyTitle?: string;
}) {
  const t = useTheme();
  const editor = useRef<HTMLTextAreaElement>(null);
  // A new note puts the cursor straight in the editor.
  useEffect(() => {
    if (selectedId === 'new') editor.current?.focus();
  }, [selectedId]);
  const open = selectedId !== null;
  return (
    <div
      data-testid="notes"
      style={{
        display: 'grid',
        gridTemplateColumns: compact ? '1fr' : '200px minmax(0, 1fr)',
        gridTemplateRows: compact ? 'auto minmax(0, 1fr)' : undefined,
        border: `1px solid ${t.color.border}`,
        borderRadius: t.radius.md,
        minHeight: compact ? 220 : 420,
        height: compact ? 260 : 'calc(100vh - 260px)',
        minWidth: 0,
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: t.space[2],
          padding: t.space[2],
          minHeight: 0,
          borderRight: compact ? undefined : `1px solid ${t.color.border}`,
          borderBottom: compact ? `1px solid ${t.color.border}` : undefined,
          maxHeight: compact ? 110 : undefined,
        }}
      >
        <div style={{ display: 'flex', gap: t.space[1] }}>
          <Input
            placeholder="search notes"
            aria-label="search notes"
            value={query}
            onChange={(e) => onQuery(e.target.value)}
            data-testid="notes-search"
            style={{ height: 28 }}
          />
          <IconButton
            icon="edit"
            label="new note"
            onClick={onNew}
            data-testid="note-new"
          />
        </div>
        <ul
          aria-label="notes"
          style={{
            listStyle: 'none',
            margin: 0,
            padding: 0,
            overflow: 'auto',
            minHeight: 0,
            flex: 1,
          }}
        >
          {notes.length === 0 ? (
            <li>
              <Text variant="caption" muted>
                {query ? 'no matches' : 'no notes'}
              </Text>
            </li>
          ) : (
            notes.map((n) => (
              <ListRow
                key={n.id}
                title={n.title}
                meta={n.meta}
                selected={n.id === selectedId}
                onSelect={() => onSelect(n.id)}
                data-testid="note"
              />
            ))
          )}
        </ul>
      </div>
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          minWidth: 0,
          minHeight: 0,
        }}
      >
        {open ? (
          <>
            <textarea
              ref={editor}
              aria-label={editorLabel}
              data-testid="note-editor"
              value={draft}
              maxLength={4000}
              placeholder="start typing"
              onChange={(e) => onDraft(e.target.value)}
              spellCheck
              style={{
                flex: 1,
                minHeight: 0,
                resize: 'none',
                border: 'none',
                outline: 'none',
                background: 'transparent',
                color: t.color.text,
                padding: t.space[4],
                ...typeStyle('body', t),
              }}
            />
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: t.space[2],
                padding: `${t.space[1]} ${t.space[2]}`,
                borderTop: `1px solid ${t.color.border}`,
              }}
            >
              {error ? (
                <Text variant="caption" tone="danger" role="alert">
                  {error}
                </Text>
              ) : (
                <Text variant="caption" muted data-testid="note-status">
                  {status}
                </Text>
              )}
              <IconButton
                icon="trash"
                label="delete note"
                tone="danger"
                onClick={onDelete}
                data-testid="note-delete"
              />
            </div>
          </>
        ) : (
          <EmptyState title={emptyTitle} icon="edit" />
        )}
      </div>
    </div>
  );
}
