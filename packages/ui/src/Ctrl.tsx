import {
  ModelRequired,
  ModelRequirement,
  type ModelRequirementState,
} from './ModelRequirement';
import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { typeStyle, tokens } from './tokens';
import { Text } from './Text';
import { Badge, type BadgeTone } from './Badge';
import { Button } from './Button';
import { Callout } from './Callout';
import { Icon } from './Icon';

/** ctrl (ADR 0103): the views' building blocks. Data in, handlers out; all copy lowercase. */

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
      <ol
        style={{
          margin: 0,
          padding: 0,
          display: 'grid',
          gap: tokens.space[0.5],
        }}
      >
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
              // Events after the scrub position: the muted text colour only (no opacity, which failed contrast).
              color: i < at ? t.color.text : t.color.textMuted,
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

export interface DecisionOptionItem {
  id: string;
  label: string;
  /** Calibrated probability, 0..1. */
  p: number;
  chosen?: boolean;
  /** Can't be undone: the confirm gate asks before it runs. */
  irreversible?: boolean;
}

export interface DecisionItem {
  id: string;
  /** "what kind of task", "which app", …. */
  question: string;
  outcome: 'act' | 'tentative' | 'ask';
  answered?: boolean;
  /** "93% · qrtz-heuristic 1". */
  meta: string;
  options: DecisionOptionItem[];
}

const OUTCOME: Record<
  DecisionItem['outcome'] | 'answered',
  [string, BadgeTone]
> = {
  act: ['sure', 'success'],
  tentative: ['tentative', 'warning'],
  ask: ['asked you', 'warning'],
  answered: ['you chose', 'primary'],
};

/** One option with its probability bar. */
function OptionBar({ o }: { o: DecisionOptionItem }) {
  const t = useTheme();
  const pct = Math.round(Math.max(0, Math.min(1, o.p)) * 100);
  return (
    <li
      data-testid="ctrl-option"
      data-chosen={o.chosen || undefined}
      style={{
        listStyle: 'none',
        display: 'grid',
        gridTemplateColumns: `minmax(0, 1fr) ${tokens.size.ctrlScore}px ${tokens.size.ctrlPct}px`,
        alignItems: 'center',
        gap: t.space[2],
        ...typeStyle('caption', t),
        color: o.chosen ? t.color.text : t.color.textMuted,
      }}
    >
      <span
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: t.space[1],
          minWidth: 0,
        }}
      >
        {o.chosen ? <Icon name="check" size="xs" /> : null}
        <span data-testid="ctrl-option-label">{o.label}</span>
        {o.irreversible ? (
          <Icon
            name="alert"
            size="xs"
            tone="warning"
            label="can't be undone"
            data-testid="ctrl-option-irreversible"
          />
        ) : null}
      </span>
      <span
        aria-hidden
        style={{
          height: tokens.size.bar,
          borderRadius: t.radius.full,
          background: t.color.border,
          overflow: 'hidden',
        }}
      >
        <span
          style={{
            display: 'block',
            width: `${pct}%`,
            height: '100%',
            background: o.chosen ? t.color.primary : t.color.textMuted,
          }}
        />
      </span>
      <span style={{ textAlign: 'right' }}>{pct}%</span>
    </li>
  );
}

/**
 * The decision inspector (decision-layer spec §13.7): each small decision of a run, its options with probability
 * bars (the top few plus the chosen one), the outcome and the model. Data in; nothing here decides.
 */
export function DecisionList({
  decisions,
  shown = 4,
}: {
  decisions: DecisionItem[];
  /** Options per decision before the rest are summarized. */
  shown?: number;
}) {
  const t = useTheme();
  return (
    <ol
      aria-label="decisions"
      data-testid="ctrl-decisions"
      style={{ margin: 0, padding: 0, display: 'grid', gap: t.space[3] }}
    >
      {decisions.map((d) => {
        const ranked = [...d.options].sort((a, b) => b.p - a.p);
        const top = ranked.slice(0, shown);
        const chosen = ranked.find((o) => o.chosen);
        if (chosen && !top.includes(chosen)) top.push(chosen);
        const [word, tone] = OUTCOME[d.answered ? 'answered' : d.outcome];
        return (
          <li
            key={d.id}
            data-testid="ctrl-decision"
            data-outcome={d.answered ? 'answered' : d.outcome}
            style={{ listStyle: 'none', display: 'grid', gap: t.space[1] }}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: t.space[2],
              }}
            >
              <Text variant="label" data-testid="ctrl-decision-question">
                {d.question}
              </Text>
              <Badge tone={tone} data-testid="ctrl-decision-outcome">
                {word}
              </Badge>
              <Text variant="caption" muted>
                {d.meta}
              </Text>
            </div>
            <ul
              style={{
                margin: 0,
                padding: 0,
                display: 'grid',
                gap: tokens.space[0.5],
              }}
            >
              {top.map((o) => (
                <OptionBar key={o.id} o={o} />
              ))}
            </ul>
            {d.options.length > top.length ? (
              <Text variant="caption" muted>
                {d.options.length - top.length} more options
              </Text>
            ) : null}
          </li>
        );
      })}
    </ol>
  );
}

/** A decision layer wasn't sure: "which one: …" with its top options, or cancel the run. */
export function AskCard({
  question,
  options,
  onPick,
  onCancel,
}: {
  question: string;
  options: { id: string; label: string }[];
  onPick: (id: string) => void;
  onCancel: () => void;
}) {
  return (
    <Callout
      tone="info"
      role="group"
      aria-label="ctrl needs you"
      data-testid="ctrl-ask"
      title={/\?\s*$/.test(question) ? question : `${question}: which one?`}
    >
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: tokens.space[2],
          marginTop: tokens.space[1],
        }}
      >
        {options.map((o) => (
          <Button
            key={o.id}
            size="sm"
            variant="secondary"
            onClick={() => onPick(o.id)}
            data-testid="ctrl-ask-option"
          >
            {o.label}
          </Button>
        ))}
        <Button
          size="sm"
          variant="ghost"
          onClick={onCancel}
          data-testid="ctrl-ask-cancel"
        >
          cancel run
        </Button>
      </div>
    </Callout>
  );
}

/**
 * The confirm gate (ADR 0214): the host's rules matched (a send, a purchase, a deletion, a quit, a system settings
 * change). The ok covers exactly this action on this element in this window; if the window changes before ctrl
 * acts, it asks again. A card in the ctrl view, never a modal.
 */
export function ConfirmCard({
  action,
  app,
  rules,
  onApprove,
  onDeny,
}: {
  action: string;
  app: string | null;
  rules: string[];
  onApprove: () => void;
  onDeny: () => void;
}) {
  return (
    <Callout
      tone="warning"
      role="group"
      aria-label="ctrl needs your ok"
      data-testid="ctrl-confirm"
      title={
        <span data-testid="ctrl-confirm-action">
          {action}
          {app ? ` · ${app}` : ''}
        </span>
      }
    >
      <span data-testid="ctrl-confirm-rules">
        asks first because: {rules.map(ruleLabel).join(', ')}
      </span>
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: tokens.space[2],
          marginTop: tokens.space[2],
        }}
      >
        <Button
          variant="primary"
          size="sm"
          onClick={onApprove}
          data-testid="ctrl-confirm-approve"
        >
          do it once
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={onDeny}
          data-testid="ctrl-confirm-deny"
        >
          don't
        </Button>
      </div>
    </Callout>
  );
}

/** A host rule id (`label:send`, `app:system_settings`, `key:quit`) as copy. */
export function ruleLabel(rule: string): string {
  const [kind, what = ''] = rule.split(':');
  const w = what.replace(/_/g, ' ');
  if (kind === 'label')
    return w === 'mixed script' ? 'a label mixing alphabets' : `it says "${w}"`;
  if (kind === 'app')
    return w === 'system settings'
      ? 'it changes a system setting'
      : `it ${w.replace('submit', 'sends')}`;
  if (kind === 'key') return `the shortcut ${w}s`;
  return w || rule;
}

/**
 * While ctrl acts in the foreground: esc stops it at once, and so does any key, click or scroll of yours.
 */
export function StopBar({ onStop }: { onStop: () => void }) {
  return (
    <Callout
      tone="info"
      data-testid="ctrl-stopbar"
      title="ctrl is using your screen · press esc or touch any key to stop"
      action={
        <Button
          size="sm"
          variant="secondary"
          onClick={onStop}
          data-testid="ctrl-stop"
        >
          stop
        </Button>
      }
    />
  );
}

export interface DecisionModelInfo {
  consented: boolean;
  state:
    | 'off'
    | 'unavailable'
    | 'not_installed'
    | 'downloading'
    | 'installed'
    | 'failed';
  sizeBytes: number;
  scorer: 'heuristic' | 'laya';
  error: string | null;
}

const MODEL_STATE: Record<DecisionModelInfo['state'], ModelRequirementState> = {
  off: 'missing',
  not_installed: 'missing',
  unavailable: 'unavailable',
  downloading: 'downloading',
  installed: 'ready',
  failed: 'error',
};

const MODEL_PROPS = {
  name: 'decision model',
  purpose: "picks ctrl's next step, on this mac",
} as const;

const modelError = (model: DecisionModelInfo) =>
  model.state === 'failed'
    ? (model.error ?? 'failed its check, nothing was kept')
    : null;

/**
 * ctrl's decision model (ADR 0116), once it's on this mac: its size and "turn off". The only time ctrl uses the
 * network is its download, and only after a yes in CtrlModelRequired.
 */
export function DecisionModelCard({
  model,
  onConsent,
}: {
  model: DecisionModelInfo;
  onConsent: (yes: boolean) => void;
}) {
  return (
    <ModelRequirement
      {...MODEL_PROPS}
      sizeBytes={model.sizeBytes || null}
      state={MODEL_STATE[model.state]}
      error={modelError(model)}
      {...(model.consented
        ? { onRemove: () => onConsent(false), onCancel: () => onConsent(false) }
        : { onDownload: () => onConsent(true) })}
      removeLabel="turn off"
      data-testid="ctrl-model"
      testIds={{
        download: 'ctrl-model-on',
        remove: 'ctrl-model-off',
        cancel: 'ctrl-model-off',
        status: 'ctrl-model-line',
      }}
    />
  );
}

/**
 * ctrl can't decide without its model: until it's installed, ctrl's page shows the ModelRequired banner and the
 * rest of the page blurred and inert. Whatever the backend reports as its scorer, a model that isn't installed
 * blocks.
 */
export function CtrlModelRequired({
  model,
  onConsent,
  children,
}: {
  model: DecisionModelInfo;
  onConsent: (yes: boolean) => void;
  children?: ReactNode;
}) {
  return (
    <ModelRequired
      app="ctrl"
      {...MODEL_PROPS}
      sizeBytes={model.sizeBytes || null}
      state={MODEL_STATE[model.state]}
      error={modelError(model)}
      onDownload={() => onConsent(true)}
      onRetry={() => onConsent(true)}
      {...(model.consented ? { onCancel: () => onConsent(false) } : {})}
      data-testid="ctrl-model"
      testIds={{
        download: 'ctrl-model-on',
        retry: 'ctrl-model-retry',
        cancel: 'ctrl-model-off',
        status: 'ctrl-model-line',
      }}
    >
      {children}
    </ModelRequired>
  );
}
