import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { transition, typeStyle } from './tokens';
import { Text } from './Text';
import { Icon } from './Icon';
import { IconButton } from './Layout';

/** Five-bar live mic level (qrtz talk pill). */
export function LevelBars({ level }: { level: number }) {
  const t = useTheme();
  const shape = [0.45, 0.8, 1, 0.8, 0.45];
  return (
    <span
      aria-hidden
      data-testid="talk-level"
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 2,
        height: 18,
      }}
    >
      {shape.map((k, i) => (
        <span
          key={i}
          style={{
            width: 3,
            height: `${Math.max(3, Math.round(18 * Math.min(1, level * k * 1.4)))}px`,
            borderRadius: 2,
            background: t.color.text,
            transition: 'height 80ms linear',
          }}
        />
      ))}
    </span>
  );
}

export type TalkPillPhase =
  | 'idle'
  | 'listening'
  | 'finalizing'
  | 'inserting'
  | 'error';

/**
 * Floating dictation pill: level bars and the streaming text (committed + dimmed partial), one line, newest words
 * visible. Click-through window; purely informational.
 */
export function TalkPill({
  phase,
  level,
  text,
  partial,
  error,
  inline,
}: {
  /** Inside a page (fixed height) rather than filling the floating pill window. */
  inline?: boolean;
  phase: TalkPillPhase;
  level: number;
  text: string;
  partial: string;
  error?: string;
}) {
  const t = useTheme();
  const busy = phase === 'finalizing' || phase === 'inserting';
  return (
    <div
      data-testid="talk-pill"
      data-phase={phase}
      role="status"
      aria-live="polite"
      style={{
        boxSizing: 'border-box',
        height: inline ? 44 : '100vh',
        display: 'grid',
        gridTemplateColumns: 'auto 1fr',
        alignItems: 'center',
        gap: t.space[3],
        padding: `0 ${t.space[4]}`,
        background: t.color.surface,
        border: `1px solid ${t.color.borderStrong}`,
        borderRadius: 999,
        overflow: 'hidden',
      }}
    >
      {phase === 'error' ? (
        <Icon name="alert" size="sm" />
      ) : busy ? (
        <span style={{ opacity: 0.7, display: 'inline-flex' }}>
          <Icon name="more" size="sm" />
        </span>
      ) : (
        <LevelBars level={level} />
      )}
      <div
        style={{
          minWidth: 0,
          overflow: 'hidden',
          whiteSpace: 'nowrap',
          direction: 'rtl',
          textAlign: 'left',
          ...typeStyle('body', t),
        }}
      >
        <span
          data-testid="talk-pill-text"
          style={{ direction: 'ltr', unicodeBidi: 'plaintext' }}
        >
          {phase === 'error' ? (
            error
          ) : (
            <>
              {text}
              {text && partial ? ' ' : ''}
              <span style={{ color: t.color.textMuted }}>{partial}</span>
              {!text && !partial ? (
                <span style={{ color: t.color.textMuted }}>…</span>
              ) : null}
            </>
          )}
        </span>
      </div>
    </div>
  );
}

/** One dictation in the talk history: the text, a quiet meta line, copy/delete. */
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
  return (
    <li
      data-testid="talk-entry"
      style={{
        listStyle: 'none',
        display: 'grid',
        gridTemplateColumns: '1fr auto',
        gap: t.space[3],
        padding: `${t.space[3]} 0`,
        borderBottom: `1px solid ${t.color.border}`,
        transition: transition('background', 'hover', t),
      }}
    >
      <div style={{ display: 'grid', gap: t.space[1], minWidth: 0 }}>
        <Text data-testid="talk-entry-text">{text}</Text>
        <Text variant="caption" muted data-testid="talk-entry-meta">
          {meta}
          {flag ? ` · ${flag}` : ''}
        </Text>
      </div>
      <div style={{ display: 'flex', gap: t.space[1], alignItems: 'start' }}>
        <IconButton
          icon="copy"
          label="copy"
          onClick={onCopy}
          data-testid="talk-entry-copy"
        />
        <IconButton
          icon="trash"
          label="delete"
          onClick={onDelete}
          data-testid="talk-entry-delete"
        />
      </div>
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
        padding: `2px ${t.space[1]} 2px ${t.space[2]}`,
        border: `1px solid ${t.color.border}`,
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
