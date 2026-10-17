import {
  forwardRef,
  memo,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { useTheme } from './theme';
import { Text } from './Text';
import { Icon, type IconName } from './Icon';
import { Button } from './Button';
import { Input } from './Input';
import { Dialog } from './Overlay';
import { IconButton } from './Layout';
import { transition, typeStyle } from './tokens';

/** One transcript line (memoized; content-visibility keeps 2-hour transcripts fast). */
export const TranscriptRow = memo(function TranscriptRow({
  time,
  speaker,
  text,
  active,
  onSeek,
}: {
  time: string;
  speaker: string;
  text: string;
  active: boolean;
  onSeek: () => void;
}) {
  const t = useTheme();
  return (
    <div
      role="listitem"
      tabIndex={0}
      aria-current={active || undefined}
      aria-label={`${time} ${speaker}: ${text}`}
      data-testid="transcript-segment"
      data-active={active || undefined}
      onClick={onSeek}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onSeek();
        }
      }}
      style={{
        display: 'grid',
        gridTemplateColumns: '56px 1fr',
        gap: t.space[2],
        padding: `${t.space[1]} ${t.space[2]}`,
        borderRadius: t.radius.md,
        cursor: 'pointer',
        background: active ? t.color.surfaceHover : 'transparent',
        contentVisibility: 'auto',
        containIntrinsicSize: 'auto 56px',
      }}
    >
      <Text variant="mono" muted>
        {time}
      </Text>
      <div>
        <Text variant="label" tone="muted">
          {speaker}
        </Text>
        <Text as="p">{text}</Text>
      </div>
    </div>
  );
});

export function TranscriptList({ children }: { children: ReactNode }) {
  return (
    <div
      data-testid="detail-transcript"
      role="list"
      aria-label="transcript"
      style={{ display: 'grid', gap: 2 }}
    >
      {children}
    </div>
  );
}

export interface NoteItem {
  id: string;
  time: string;
  text: string;
}

/**
 * Granola-style notes editor: one input, timestamped lines below. Enter adds; click a note to edit,
 * empty it to delete. Pure UI: the caller persists through onAdd/onUpdate.
 */
export function NotesEditor({
  notes,
  onAdd,
  onUpdate,
  compact,
  placeholder = 'Note…',
  error,
}: {
  notes: NoteItem[];
  onAdd: (text: string) => void;
  onUpdate: (id: string, text: string) => void;
  compact?: boolean;
  placeholder?: string;
  error?: string | null;
}) {
  const t = useTheme();
  const [draft, setDraft] = useState('');
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(
    null,
  );
  const save = () => {
    if (!editing) return;
    onUpdate(editing.id, editing.text.trim());
    setEditing(null);
  };
  return (
    <div data-testid="notes" style={{ display: 'grid', gap: t.space[2] }}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          const text = draft.trim();
          if (!text) return;
          setDraft('');
          onAdd(text);
        }}
        style={{ display: 'flex', gap: t.space[2] }}
      >
        <Input
          aria-label="add a note"
          placeholder={placeholder}
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
          add
        </Button>
      </form>
      {error ? (
        <Text variant="bodySm" tone="danger" role="alert">
          {error}
        </Text>
      ) : null}
      <ul
        aria-label="notes"
        style={{
          listStyle: 'none',
          margin: 0,
          padding: 0,
          display: 'grid',
          gap: t.space[1],
          ...(compact ? { maxHeight: 140, overflow: 'auto' } : {}),
        }}
      >
        {notes.map((n) => (
          <li
            key={n.id}
            data-testid="note"
            style={{ display: 'flex', gap: t.space[2], alignItems: 'baseline' }}
          >
            <Text variant="mono" muted data-testid="note-time">
              {n.time}
            </Text>
            {editing?.id === n.id ? (
              <Input
                autoFocus
                aria-label="edit note"
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
                  ...typeStyle('body', t),
                  color: t.color.text,
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

/** Summary, action items and Q&A. */
export function SummaryBlock({
  summary,
  actionItems,
  qa,
  toolbar,
  notice,
  source,
}: {
  summary: string;
  actionItems: { text: string; owner?: string }[];
  qa: { question: string; answer: string }[];
  toolbar?: ReactNode;
  notice?: ReactNode;
  source?: string;
}) {
  const t = useTheme();
  return (
    <div
      data-testid="detail-summary"
      style={{ display: 'grid', gap: t.space[5] }}
    >
      {toolbar}
      {notice}
      <Text as="p">{summary}</Text>
      {actionItems.length ? (
        <div style={{ display: 'grid', gap: t.space[2] }}>
          <Text variant="label" tone="muted">
            action items
          </Text>
          {actionItems.map((a, i) => (
            <div
              key={i}
              data-testid="action-item"
              style={{
                display: 'flex',
                gap: t.space[2],
                alignItems: 'center',
              }}
            >
              <Icon name="todo" size="sm" tone="muted" />
              <Text style={{ flex: 1 }}>{a.text}</Text>
              {a.owner ? (
                <Text variant="caption" muted>
                  {a.owner}
                </Text>
              ) : null}
            </div>
          ))}
        </div>
      ) : null}
      {qa.length ? (
        <div style={{ display: 'grid', gap: t.space[2] }}>
          <Text variant="label" tone="muted">
            q&amp;a
          </Text>
          {qa.map((q, i) => (
            <div key={i} style={{ display: 'grid', gap: 2 }}>
              <Text>{q.question}</Text>
              <Text muted>{q.answer}</Text>
            </div>
          ))}
        </div>
      ) : null}
      {source ? (
        <Text variant="caption" muted>
          {source}
        </Text>
      ) : null}
    </div>
  );
}

/** One-line input + submit button. */
export function Composer({
  value,
  onChange,
  onSubmit,
  placeholder,
  label,
  busy,
  submitLabel = 'Ask',
  inputTestId,
  submitTestId,
}: {
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  placeholder: string;
  label: string;
  busy?: boolean;
  submitLabel?: string;
  inputTestId?: string;
  submitTestId?: string;
}) {
  const t = useTheme();
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit();
      }}
      style={{ display: 'flex', gap: t.space[2] }}
    >
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        aria-label={label}
        data-testid={inputTestId}
        maxLength={2000}
      />
      <Button
        type="submit"
        disabled={busy || !value.trim()}
        data-testid={submitTestId}
      >
        {submitLabel}
      </Button>
    </form>
  );
}

/** Pulsing record dot. */
export function RecordDot({ on = true }: { on?: boolean }) {
  const t = useTheme();
  return (
    <span
      aria-hidden
      style={{
        width: 8,
        height: 8,
        flexShrink: 0,
        borderRadius: 4,
        background: on ? t.color.recording : t.color.textSubtle,
        animation: on
          ? `q-pulse ${t.motion.pulse} ease-in-out infinite`
          : undefined,
      }}
    />
  );
}

/** Thin input-level meter. */
export function LevelMeter({ level }: { level: number }) {
  const t = useTheme();
  return (
    <span
      aria-hidden
      style={{
        width: 48,
        height: 3,
        background: t.color.border,
        borderRadius: 2,
        overflow: 'hidden',
        display: 'inline-block',
      }}
    >
      <span
        style={{
          display: 'block',
          width: `${Math.round(level * 100)}%`,
          height: '100%',
          background: t.color.text,
          transition: 'width 120ms linear',
        }}
      />
    </span>
  );
}

/** Minimal recording strip: dot · clock · title · speaker · pause/stop. */
export function RecordingBar({
  clock,
  title,
  level,
  speaker,
  paused,
  onPause,
  onStop,
}: {
  clock: string;
  title: string;
  level: number;
  speaker?: string | null;
  paused?: boolean;
  onPause?: () => void;
  onStop: () => void;
}) {
  const t = useTheme();
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: t.space[3] }}>
      <RecordDot on={!paused} />
      <Text variant="mono">{clock}</Text>
      <Text muted truncate>
        {title}
      </Text>
      <LevelMeter level={level} />
      {speaker ? (
        <Text variant="caption" muted data-testid="note-active-speaker">
          {speaker}
        </Text>
      ) : null}
      {paused ? (
        <Text variant="caption" tone="warning" data-testid="note-paused">
          paused
        </Text>
      ) : null}
      <div style={{ marginLeft: 'auto', display: 'flex', gap: t.space[2] }}>
        {onPause ? (
          <IconButton
            icon={paused ? 'play' : 'pause'}
            label={paused ? 'resume' : 'pause'}
            pressed={!!paused}
            onClick={onPause}
            data-testid="note-pause"
          />
        ) : null}
        <IconButton
          icon="stop"
          label="stop"
          tone="record"
          onClick={onStop}
          data-testid="note-stop"
        />
      </div>
    </div>
  );
}

/** Thin banner row (detected meeting, ready, failed) with trailing actions. */
export function Banner({
  children,
  actions,
  tone = 'default',
  'data-testid': testId,
}: {
  children: ReactNode;
  actions?: ReactNode;
  tone?: 'default' | 'danger';
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div
      data-testid={testId}
      role={tone === 'danger' ? 'alert' : 'status'}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: t.space[2],
        flexWrap: 'wrap',
        padding: `${t.space[2]} ${t.space[3]}`,
        border: `1px solid ${tone === 'danger' ? t.color.danger : t.color.border}`,
        borderRadius: t.radius.md,
      }}
    >
      <div style={{ flex: 1, minWidth: 120, display: 'grid' }}>{children}</div>
      {actions}
    </div>
  );
}

/** Native audio player sized to the pane. */
export const AudioPlayer = forwardRef<
  HTMLAudioElement,
  { src: string; onTime: (ms: number) => void }
>(function AudioPlayer({ src, onTime }, ref) {
  return (
    <audio
      ref={ref}
      controls
      src={src}
      onTimeUpdate={(e) => onTime(e.currentTarget.currentTime * 1000)}
      style={{ width: '100%', height: 32 }}
    />
  );
});

/** "…" overflow menu for secondary actions. */
export function ActionMenu({
  items,
  label = 'More',
  'data-testid': testId,
  children,
}: {
  items: {
    label: string;
    onSelect: () => void;
    danger?: boolean;
    testId?: string;
  }[];
  label?: string;
  'data-testid'?: string;
  /** Extra content (e.g. a template picker) shown at the top of the menu. */
  children?: ReactNode;
}) {
  const t = useTheme();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) =>
      e.key === 'Escape' && !ref.current?.closest('[inert]') && setOpen(false);
    window.addEventListener('mousedown', close);
    window.addEventListener('keydown', esc);
    return () => {
      window.removeEventListener('mousedown', close);
      window.removeEventListener('keydown', esc);
    };
  }, [open]);
  return (
    <div ref={ref} style={{ position: 'relative' }}>
      <IconButton
        icon="more"
        label={label}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        {...(testId ? { 'data-testid': testId } : {})}
      />
      {open ? (
        <div
          role="menu"
          style={{
            position: 'absolute',
            right: 0,
            top: 30,
            zIndex: t.z.overlay,
            minWidth: 200,
            display: 'grid',
            gap: 2,
            padding: t.space[1],
            background: t.color.surface,
            border: `1px solid ${t.color.borderStrong}`,
            borderRadius: t.radius.lg,
          }}
        >
          {children ? (
            <div style={{ padding: t.space[2] }}>{children}</div>
          ) : null}
          {items.map((it) => (
            <button
              key={it.label}
              type="button"
              role="menuitem"
              data-testid={it.testId}
              onClick={() => {
                setOpen(false);
                it.onSelect();
              }}
              style={{
                all: 'unset',
                cursor: 'pointer',
                padding: `${t.space[1]} ${t.space[2]}`,
                borderRadius: t.radius.sm,
                ...typeStyle('body', t),
                color: it.danger ? t.color.danger : t.color.text,
                transition: transition('background', 'hover', t),
              }}
            >
              {it.label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** Confirm dialog with Cancel / confirm buttons. */
export function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel,
  onCancel,
  onConfirm,
  'data-testid': testId,
}: {
  open: boolean;
  title: string;
  body?: ReactNode;
  confirmLabel: string;
  onCancel: () => void;
  onConfirm: () => void;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <Dialog open={open} onClose={onCancel} title={title} label={title}>
      <div data-testid={testId} style={{ display: 'grid', gap: t.space[4] }}>
        {body ? <Text muted>{body}</Text> : null}
        <div
          style={{
            display: 'flex',
            gap: t.space[2],
            justifyContent: 'flex-end',
          }}
        >
          <Button
            variant="ghost"
            onClick={onCancel}
            data-testid={testId ? `${testId}-cancel` : undefined}
          >
            cancel
          </Button>
          <Button
            variant="danger"
            onClick={onConfirm}
            data-testid={testId ? `${testId}-ok` : undefined}
          >
            {confirmLabel}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

/** ⌘K palette UI. The shell owns commands and search; this only renders and handles keys. */
export function CommandPalette({
  open,
  onClose,
  query,
  onQuery,
  results,
  index,
  onIndex,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  query: string;
  onQuery: (q: string) => void;
  results: {
    id: string;
    title: string;
    hint?: string;
    icon?: IconName;
    testId?: string;
  }[];
  index: number;
  onIndex: (i: number) => void;
  onPick: (i: number) => void;
}) {
  const t = useTheme();
  return (
    <Dialog
      open={open}
      onClose={onClose}
      width={560}
      top="14vh"
      label="command palette"
    >
      <div data-testid="palette">
        <input
          data-testid="palette-input"
          aria-label="command or meeting search"
          role="combobox"
          aria-expanded={results.length > 0}
          aria-controls="palette-results"
          aria-autocomplete="list"
          {...(results[index]
            ? { 'aria-activedescendant': `palette-option-${index}` }
            : {})}
          value={query}
          placeholder="type a command…"
          onChange={(e) => onQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown') {
              e.preventDefault();
              onIndex(Math.min(index + 1, results.length - 1));
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              onIndex(Math.max(index - 1, 0));
            } else if (e.key === 'Enter') {
              e.preventDefault();
              onPick(index);
            }
          }}
          style={{
            width: '100%',
            height: 48,
            padding: `0 ${t.space[5]}`,
            border: 'none',
            borderBottom: `1px solid ${t.color.border}`,
            background: 'transparent',
            color: t.color.text,
            ...typeStyle('bodyLg', t),
            outline: 'none',
          }}
        />
        <div
          id="palette-results"
          role="listbox"
          aria-label="results"
          style={{ maxHeight: 360, overflow: 'auto', padding: t.space[1] }}
        >
          {results.map((r, i) => (
            <div
              key={r.id}
              id={`palette-option-${i}`}
              role="option"
              aria-selected={i === index}
              data-testid={r.testId ?? 'palette-item'}
              onMouseEnter={() => onIndex(i)}
              onClick={() => onPick(i)}
              style={{
                display: 'flex',
                justifyContent: 'space-between',
                gap: t.space[3],
                padding: `${t.space[2]} ${t.space[3]}`,
                borderRadius: t.radius.md,
                cursor: 'pointer',
                background: i === index ? t.color.accent : 'transparent',
              }}
            >
              <span
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: t.space[2],
                  minWidth: 0,
                }}
              >
                {r.icon ? <Icon name={r.icon} size="sm" tone="muted" /> : null}
                <Text truncate>{r.title}</Text>
              </span>
              {r.hint ? (
                <Text variant="caption" muted truncate>
                  {r.hint}
                </Text>
              ) : null}
            </div>
          ))}
        </div>
        {results.length === 0 ? (
          <div role="status" style={{ padding: t.space[3] }}>
            <Text muted>no matching commands</Text>
          </div>
        ) : null}
      </div>
    </Dialog>
  );
}

/** Permission checklist (shown only when something is missing). */
export function PermissionList({
  items,
}: {
  items: { id: string; label: string; state: string; onGrant?: () => void }[];
}) {
  const t = useTheme();
  return (
    <ul
      style={{
        listStyle: 'none',
        margin: 0,
        padding: 0,
        display: 'grid',
        gap: t.space[2],
      }}
    >
      {items.map((p) => (
        <li
          key={p.id}
          style={{ display: 'flex', alignItems: 'center', gap: t.space[3] }}
          data-testid={`perm-${p.id}`}
        >
          <Text style={{ flex: 1 }}>{p.label}</Text>
          {p.onGrant ? (
            <Button size="sm" variant="secondary" onClick={p.onGrant}>
              allow
            </Button>
          ) : (
            <Text variant="caption" muted>
              {p.state}
            </Text>
          )}
        </li>
      ))}
    </ul>
  );
}

/** Floating mini HUD window frame (drag region, compact strip, optional expanded body). */
export function MiniHudFrame({
  strip,
  body,
}: {
  strip: ReactNode;
  body?: ReactNode;
}) {
  const t = useTheme();
  return (
    <div
      style={{
        height: '100vh',
        display: 'grid',
        gridTemplateRows: body ? '56px 1fr' : '1fr',
        background: t.color.surface,
        border: `1px solid ${t.color.borderStrong}`,
        borderRadius: 12,
        overflow: 'hidden',
      }}
    >
      <div
        data-testid="mini-hud"
        className="q-drag-all"
        style={{
          display: 'grid',
          gridTemplateColumns: 'auto 1fr auto',
          alignItems: 'center',
          gap: t.space[3],
          padding: `0 ${t.space[3]}`,
          overflow: 'hidden',
        }}
      >
        {strip}
      </div>
      {body ? (
        <div
          className="q-no-drag"
          data-testid="mini-hud-notes"
          style={{
            display: 'grid',
            gridTemplateRows: '1fr auto',
            gap: t.space[2],
            padding: `0 ${t.space[3]} ${t.space[3]}`,
            minHeight: 0,
          }}
        >
          {body}
        </div>
      ) : null}
    </div>
  );
}

/** Non-draggable group inside a drag region (buttons in the mini HUD). */
export function NoDrag({ children }: { children: ReactNode }) {
  const t = useTheme();
  return (
    <div
      className="q-no-drag"
      style={{ display: 'flex', alignItems: 'center', gap: t.space[2] }}
    >
      {children}
    </div>
  );
}

/** Two-line text cell that truncates (mini HUD status + last line). */
export function StatusLines({
  top,
  bottom,
  bottomTestId,
}: {
  top: ReactNode;
  bottom: ReactNode;
  bottomTestId?: string;
}) {
  return (
    <div style={{ minWidth: 0, display: 'grid' }}>
      <Text variant="caption" muted truncate>
        {top}
      </Text>
      <Text truncate data-testid={bottomTestId}>
        {bottom}
      </Text>
    </div>
  );
}
