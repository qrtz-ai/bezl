import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { transition, typeStyle } from './tokens';
import { Text } from './Text';
import { Icon } from './Icon';
import { IconButton } from './Layout';
import { Waveform } from './Waveform';

export type TalkPillPhase =
  | 'idle'
  | 'listening'
  | 'finalizing'
  | 'inserting'
  | 'error';

/** Screen-reader status for each pill phase (the pill draws no words outside an error). */
const PILL_STATUS: Record<TalkPillPhase, string> = {
  idle: 'echo',
  listening: 'listening',
  finalizing: 'processing',
  inserting: 'processing',
  error: 'error',
};

/**
 * Floating dictation pill: a live waveform that follows the mic while listening and relaxes to a slow swell while
 * the last words are processed. It never shows the transcript (Jared, 2026-10-17); the only words drawn are an
 * error. Click-through window; purely informational.
 */
export function TalkPill({
  phase,
  level,
  error,
  inline,
}: {
  /** Inside a page (fixed height) rather than filling the floating pill window. */
  inline?: boolean;
  phase: TalkPillPhase;
  level: number;
  error?: string;
}) {
  const t = useTheme();
  const failed = phase === 'error';
  const message = failed ? (error ?? 'something went wrong').toLowerCase() : '';
  return (
    <div
      data-testid="talk-pill"
      data-phase={phase}
      role="status"
      aria-live="polite"
      aria-label={failed ? message : PILL_STATUS[phase]}
      style={{
        boxSizing: 'border-box',
        height: inline ? 44 : '100vh',
        width: inline ? 220 : '100%',
        maxWidth: '100%',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: t.space[2],
        padding: `0 ${t.space[4]}`,
        background: t.color.surface,
        border: `1px solid ${t.color.borderStrong}`,
        borderRadius: 999,
        overflow: 'hidden',
      }}
    >
      {failed ? (
        <>
          <Icon name="alert" size="sm" />
          <span
            data-testid="talk-pill-error"
            style={{
              minWidth: 0,
              overflow: 'hidden',
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
              ...typeStyle('bodySm', t),
              color: t.color.text,
            }}
          >
            {message}
          </span>
        </>
      ) : (
        <Waveform
          level={phase === 'listening' ? level : 0}
          mode={phase === 'listening' ? 'live' : 'idle'}
          width={160}
          height={24}
        />
      )}
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
        gridTemplateColumns: 'minmax(0, 1fr) auto',
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
