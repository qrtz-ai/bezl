import { useId, type ReactNode } from 'react';
import { ActionMenu } from './ActionMenu';
import { useTheme } from './theme';
import { transition, tokens } from './tokens';
import { Text } from './Text';
import { Icon } from './Icon';

/**
 * One dictation in the talk history: the text at full width and a quiet meta line. A click (or Enter/Space)
 * copies it; right-click (or Shift+F10 / the Menu key) opens an ActionMenu with copy and delete.
 */
export function TalkHistoryRow({
  text,
  meta,
  flag,
  onCopy,
  onDelete,
}: {
  text: string;
  meta: ReactNode;
  /** Short warning shown in the meta line (e.g. "copied to clipboard"). */
  flag?: string;
  onCopy: () => void;
  onDelete: () => void;
}) {
  const t = useTheme();
  const hint = useId();
  return (
    <li
      style={{
        listStyle: 'none',
        borderBottom: `${tokens.border.hairline} solid ${t.color.border}`,
      }}
    >
      <ActionMenu
        label="entry actions"
        data-testid="talk-entry"
        actions={[
          { id: 'copy', label: 'copy', icon: 'copy', onSelect: onCopy },
          {
            id: 'delete',
            label: 'delete',
            icon: 'trash',
            danger: true,
            onSelect: onDelete,
          },
        ]}
      >
        <div
          role="button"
          tabIndex={0}
          aria-haspopup="menu"
          aria-describedby={hint}
          data-testid="talk-entry"
          onClick={onCopy}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onCopy();
            }
          }}
          style={{
            display: 'grid',
            gap: t.space[1],
            minWidth: 0,
            padding: `${t.space[3]} 0`,
            cursor: 'pointer',
            transition: transition('background', 'hover', t),
          }}
        >
          <Text data-testid="talk-entry-text">{text}</Text>
          <Text variant="caption" muted data-testid="talk-entry-meta">
            {meta}
            {flag ? ` · ${flag}` : ''}
          </Text>
          <span id={hint} hidden>
            click to copy, right-click for more
          </span>
        </div>
      </ActionMenu>
    </li>
  );
}

/** Plain list container for TalkHistoryRow / DictionaryTerm. */
export function PlainList({
  children,
  label,
  'data-testid': testId,
}: {
  children: ReactNode;
  label: string;
  'data-testid'?: string;
}) {
  return (
    <ul
      aria-label={label}
      data-testid={testId}
      style={{ margin: 0, padding: 0, display: 'grid' }}
    >
      {children}
    </ul>
  );
}

/** A personal-dictionary term as a removable chip ("qrtz ← cue arts"). */
export function DictionaryTerm({
  term,
  aliases,
  onRemove,
}: {
  term: string;
  aliases: string[];
  onRemove: () => void;
}) {
  const t = useTheme();
  return (
    <li
      data-testid="talk-term"
      style={{
        listStyle: 'none',
        display: 'inline-flex',
        alignItems: 'center',
        gap: t.space[2],
        padding: `${tokens.space[0.5]} ${t.space[1]} ${tokens.space[0.5]} ${t.space[2]}`,
        border: `${tokens.border.hairline} solid ${t.color.border}`,
        borderRadius: t.radius.sm,
      }}
    >
      <Text variant="caption">{term}</Text>
      {aliases.length ? (
        <Text variant="caption" muted>
          {aliases.join(', ')}
        </Text>
      ) : null}
      <button
        type="button"
        aria-label={`remove ${term}`}
        onClick={onRemove}
        style={{
          all: 'unset',
          cursor: 'pointer',
          display: 'inline-flex',
          color: t.color.textMuted,
        }}
      >
        <Icon name="close" size="sm" />
      </button>
    </li>
  );
}

/** Wrapping row of DictionaryTerm chips. */
export function TermList({ children }: { children: ReactNode }) {
  const t = useTheme();
  return (
    <ul
      aria-label="dictionary"
      style={{
        margin: 0,
        padding: 0,
        display: 'flex',
        flexWrap: 'wrap',
        gap: t.space[2],
      }}
    >
      {children}
    </ul>
  );
}
