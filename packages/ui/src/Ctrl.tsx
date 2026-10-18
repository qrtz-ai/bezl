import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { transition, typeStyle } from './tokens';
import { Text } from './Text';
import { Badge, type BadgeTone } from './Badge';
import { IconButton, Chip } from './Layout';
import { Input, Select } from './Input';

/** ctrl (ADR 0103): the views' building blocks. Data in, handlers out; all copy lowercase. */

export type ScheduleKind = 'once' | 'daily' | 'weekly';
export const WEEKDAYS = [
  'sunday',
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
] as const;
const KINDS: ScheduleKind[] = ['once', 'daily', 'weekly'];

/**
 * When a task runs: once / daily / weekly, a time (hh:mm, local), and the day for weekly. For `once` an empty
 * time means as soon as possible.
 */
export function SchedulePicker({
  kind,
  time,
  weekday,
  onKind,
  onTime,
  onWeekday,
}: {
  kind: ScheduleKind;
  /** `hh:mm` (24 h) or '' (once only: as soon as possible). */
  time: string;
  weekday: number;
  onKind: (k: ScheduleKind) => void;
  onTime: (t: string) => void;
  onWeekday: (d: number) => void;
}) {
  const t = useTheme();
  return (
    <div
      role="group"
      aria-label="schedule"
      data-testid="ctrl-schedule"
      data-kind={kind}
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        alignItems: 'center',
        gap: t.space[2],
      }}
    >
      {KINDS.map((k) => (
        <Chip
          key={k}
          pressed={k === kind}
          onClick={() => onKind(k)}
          data-testid={`ctrl-schedule-${k}`}
        >
          {k}
        </Chip>
      ))}
      {kind === 'weekly' ? (
        <Select
          aria-label="day"
          value={String(weekday)}
          onChange={(e) => onWeekday(Number(e.target.value))}
          data-testid="ctrl-schedule-day"
          style={{ width: 130 }}
        >
          {WEEKDAYS.map((d, i) => (
            <option key={d} value={i}>
              {d}
            </option>
          ))}
        </Select>
      ) : null}
      <Input
        type="time"
        aria-label={kind === 'once' ? 'time (empty = now)' : 'time'}
        value={time}
        required={kind !== 'once'}
        onChange={(e) => onTime(e.target.value)}
        data-testid="ctrl-schedule-time"
        style={{ width: 120 }}
      />
      {kind === 'once' && !time ? (
        <Text variant="caption" muted>
          as soon as possible
        </Text>
      ) : null}
    </div>
  );
}

/** One task: what was typed, when it runs, and run / pause / delete. Selecting it shows its runs. */
export function CtrlTaskRow({
  text,
  schedule,
  meta,
  paused,
  selected,
  onSelect,
  onRun,
  onPause,
  onDelete,
}: {
  text: string;
  /** "every monday at 09:30". */
  schedule: string;
  meta?: ReactNode;
  paused: boolean;
  selected?: boolean;
  onSelect: () => void;
  onRun: () => void;
  onPause: () => void;
  onDelete: () => void;
}) {
  const t = useTheme();
  return (
    <li
      data-testid="ctrl-task"
      data-paused={paused || undefined}
      aria-current={selected || undefined}
      style={{
        listStyle: 'none',
        display: 'grid',
        gridTemplateColumns: '1fr auto',
        gap: t.space[3],
        padding: `${t.space[2]} ${t.space[3]}`,
        borderRadius: t.radius.md,
        background: selected ? t.color.accent : 'transparent',
        transition: transition('background', 'hover', t),
      }}
    >
      <button
        type="button"
        onClick={onSelect}
        data-testid="ctrl-task-select"
        style={{
          all: 'unset',
          cursor: 'pointer',
          minWidth: 0,
          display: 'grid',
          gap: t.space[1],
        }}
      >
        <Text data-testid="ctrl-task-text">{text}</Text>
        <Text variant="caption" muted data-testid="ctrl-task-schedule">
          {paused ? `paused · ${schedule}` : schedule}
          {meta ? <> · {meta}</> : null}
        </Text>
      </button>
      <div style={{ display: 'flex', gap: t.space[1], alignItems: 'start' }}>
        <IconButton
          icon="play"
          label="run now"
          onClick={onRun}
          data-testid="ctrl-task-run"
        />
        <IconButton
          icon="pause"
          label={paused ? 'resume' : 'pause'}
          pressed={paused}
          onClick={onPause}
          data-testid="ctrl-task-pause"
        />
        <IconButton
          icon="trash"
          label="delete"
          onClick={onDelete}
          data-testid="ctrl-task-delete"
        />
      </div>
    </li>
  );
}

export interface RunItem {
  id: string;
  /** "pending intent", "done", …. */
  status: string;
  tone: BadgeTone;
  meta: string;
}

/** A task's runs, newest first. */
export function RunHistory({
  runs,
  selected,
  onSelect,
}: {
  runs: RunItem[];
  selected?: string | null;
  onSelect: (id: string) => void;
}) {
  const t = useTheme();
  return (
    <ul
      aria-label="runs"
      data-testid="ctrl-runs"
      style={{ margin: 0, padding: 0, display: 'grid', gap: t.space[1] }}
    >
      {runs.map((r) => (
        <li key={r.id} style={{ listStyle: 'none' }}>
          <button
            type="button"
            data-testid="ctrl-run"
            aria-current={r.id === selected || undefined}
            onClick={() => onSelect(r.id)}
            style={{
              all: 'unset',
              boxSizing: 'border-box',
              cursor: 'pointer',
              width: '100%',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: t.space[2],
              padding: `${t.space[1]} ${t.space[2]}`,
              borderRadius: t.radius.md,
              background: r.id === selected ? t.color.accent : 'transparent',
            }}
          >
            <Text variant="caption" muted>
              {r.meta}
            </Text>
            <Badge tone={r.tone} data-testid="ctrl-run-status">
              {r.status}
            </Badge>
          </button>
        </li>
      ))}
    </ul>
  );
}

export interface TimelineItem {
  seq: number;
  label: string;
  meta?: string;
}

/**
 * A run's event log with a scrubber: the state shown is the fold of the first `at` events. Moving it executes
 * nothing; events after `at` are dimmed.
 */
export function RunTimeline({
  events,
  at,
  onAt,
  summary,
}: {
  events: TimelineItem[];
  at: number;
  onAt: (n: number) => void;
  /** The state at `at`, already worded. */
  summary: ReactNode;
}) {
  const t = useTheme();
  return (
    <div
      data-testid="ctrl-timeline"
      style={{ display: 'grid', gap: t.space[2] }}
    >
      <input
        type="range"
        min={0}
        max={events.length}
        step={1}
        value={at}
        aria-label="step"
        aria-valuetext={`step ${at} of ${events.length}`}
        onChange={(e) => onAt(Number(e.target.value))}
        data-testid="ctrl-scrub"
        style={{ width: '100%', accentColor: t.color.primary }}
      />
      <Text variant="caption" muted data-testid="ctrl-state">
        step {at} of {events.length} · {summary}
      </Text>
      <ol style={{ margin: 0, padding: 0, display: 'grid', gap: 2 }}>
        {events.map((e, i) => (
          <li
            key={e.seq}
            data-testid="ctrl-event"
            data-applied={i < at || undefined}
            style={{
              listStyle: 'none',
              display: 'flex',
              gap: t.space[2],
              ...typeStyle('caption', t),
              color: i < at ? t.color.text : t.color.textMuted,
              opacity: i < at ? 1 : 0.5,
            }}
          >
            <span>{e.seq}</span>
            <span>{e.label}</span>
            {e.meta ? <span>{e.meta}</span> : null}
          </li>
        ))}
      </ol>
    </div>
  );
}
