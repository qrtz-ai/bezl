import { useEffect, useRef, type ReactNode } from 'react';
import { useTheme } from './theme';
import { Text } from './Text';
import { Input } from './Input';
import { EmptyState } from './EmptyState';
import { IconButton, ListRow } from './Layout';
import { typeStyle, tokens } from './tokens';
import type { UserError } from './UserError';

export interface NotesAppItem {
  id: string;
  /** First non-empty line of the note. */
  title: string;
  /** Short date/time label for the list. */
  meta: string;
}

/**
 * A notes app (ADR 0219): a searchable list of notes on the left, one full-height plain-text editor on the
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
  banner,
  tools,
  readOnly,
}: {
  notes: NotesAppItem[];
  /** Selected note id, `'new'` for an unsaved new note, or null when nothing is open. */
  selectedId: string | null;
  draft: string;
  query: string;
  compact?: boolean;
  /** Save state shown under the editor ("saved", "saving…"). */
  status?: ReactNode;
  /** QRT-311: a quiet footer line ("couldn't save: …"); the raw reason is only the line's tooltip. */
  error?: UserError | null;
  onQuery: (q: string) => void;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDraft: (text: string) => void;
  onDelete: () => void;
  editorLabel?: string;
  emptyTitle?: string;
  /** Above the editor (e.g. "changed outside qrtz" with its choices). */
  banner?: ReactNode;
  /** In the editor's footer, before delete (e.g. the note's meeting links). */
  tools?: ReactNode;
  /** Shown but not editable (a file qrtz won't change). */
  readOnly?: boolean;
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
        gridTemplateColumns: compact
          ? '1fr'
          : `${tokens.layout.notesSidebar}px minmax(0, 1fr)`,
        gridTemplateRows: compact ? 'auto minmax(0, 1fr)' : undefined,
        border: `${tokens.border.hairline} solid ${t.color.border}`,
        borderRadius: t.radius.md,
        minHeight: compact
          ? tokens.layout.notesCompactMinHeight
          : tokens.layout.notesMinHeight,
        height: compact
          ? tokens.layout.notesCompactHeight
          : `calc(100vh - ${tokens.layout.notesChrome}px)`,
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
          borderRight: compact
            ? undefined
            : `${tokens.border.hairline} solid ${t.color.border}`,
          borderBottom: compact
            ? `${tokens.border.hairline} solid ${t.color.border}`
            : undefined,
          maxHeight: compact ? tokens.layout.notesCompactListMax : undefined,
        }}
      >
        <div style={{ display: 'flex', gap: t.space[1] }}>
          <Input
            placeholder="search notes"
            aria-label="search notes"
            value={query}
            onChange={(e) => onQuery(e.target.value)}
            data-testid="notes-search"
            style={{ height: tokens.size.row.sm }}
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
            {banner ? (
              <div
                style={{
                  padding: t.space[2],
                  borderBottom: `${tokens.border.hairline} solid ${t.color.border}`,
                }}
              >
                {banner}
              </div>
            ) : null}
            <textarea
              ref={editor}
              aria-label={editorLabel}
              data-testid="note-editor"
              value={draft}
              readOnly={readOnly}
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
                borderTop: `${tokens.border.hairline} solid ${t.color.border}`,
              }}
            >
              {error ? (
                <Text
                  variant="caption"
                  tone="danger"
                  role="alert"
                  data-testid="notes-error"
                  {...(error.detail ? { title: error.detail } : {})}
                >
                  {error.title}: {error.next}
                </Text>
              ) : (
                <Text variant="caption" muted data-testid="note-status">
                  {status}
                </Text>
              )}
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: t.space[1],
                  minWidth: 0,
                }}
              >
                {tools}
              </div>
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
