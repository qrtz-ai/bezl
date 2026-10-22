import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { transition, typeStyle, tokens } from './tokens';
import { Text } from './Text';
import { Icon } from './Icon';
import { Waveform } from './Waveform';
import { ContextMenu } from './ContextMenu';

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
        // QRT-291: inside a page a failed pill grows to fit its reason (it wraps, never cut off).
        height: inline ? (failed ? 'auto' : 44) : '100vh',
        minHeight: inline ? 44 : undefined,
        width: inline ? (failed ? 'fit-content' : 220) : '100%',
        maxWidth: '100%',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: t.space[2],
        padding:
          inline && failed ? `${t.space[2]} ${t.space[4]}` : `0 ${t.space[4]}`,
        background: t.color.surface,
        border: `1px solid ${t.color.borderStrong}`,
        borderRadius: inline && failed ? tokens.radius.lg : tokens.radius.full,
        overflow: 'hidden',
      }}
    >
      {failed ? (
        <>
          <span style={{ flex: 'none', display: 'flex' }}>
            <Icon name="alert" size="sm" />
          </span>
          <span
            data-testid="talk-pill-error"
            style={{
              minWidth: 0,
              // Wraps onto as many lines as the reason needs ("…isn't running, so the mic can't open (echo: …)").
              whiteSpace: 'normal',
              overflowWrap: 'anywhere',
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

/**
 * One dictation in the talk history: the text at full width and a quiet meta line. A click (or Enter/Space) copies
 * the text; right-click (or the Menu key / Shift+F10) opens a context menu with copy and delete.
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
  return (
    <li
      style={{ listStyle: 'none', borderBottom: `1px solid ${t.color.border}` }}
    >
      <ContextMenu
        label="actions for this dictation"
        data-testid="talk-entry-menu"
        items={[
          {
            id: 'copy',
            label: 'copy',
            icon: 'copy',
            onSelect: onCopy,
            testId: 'talk-entry-copy',
          },
          {
            id: 'delete',
            label: 'delete',
            icon: 'trash',
            danger: true,
            onSelect: onDelete,
            testId: 'talk-entry-delete',
          },
        ]}
      >
        <div
          role="button"
          tabIndex={0}
          data-testid="talk-entry"
          aria-description="click to copy. right-click or shift+f10 for more"
          title="click to copy · right-click for more"
          onClick={onCopy}
          onKeyDown={(e) => {
            if (e.target !== e.currentTarget) return;
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
        </div>
      </ContextMenu>
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
