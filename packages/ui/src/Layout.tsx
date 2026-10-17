import {
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { useTheme } from './theme';
import { Text } from './Text';
import { transition, typeStyle, type Tokens } from './tokens';

type Space = keyof Tokens['space'];

/**
 * Flex layout from tokens. The only layout primitive feature code uses: it never takes raw CSS.
 * `as` keeps semantics (form, ul, li, nav, section, header).
 */
export function Stack({
  children,
  dir = 'col',
  gap = 2,
  align,
  justify,
  wrap,
  grow,
  pad,
  scroll,
  as: Tag = 'div',
  onSubmit,
  role,
  id,
  'aria-label': ariaLabel,
  'aria-labelledby': labelledBy,
  'aria-live': ariaLive,
  'data-testid': testId,
}: {
  children?: ReactNode;
  dir?: 'row' | 'col';
  gap?: Space;
  align?: 'start' | 'center' | 'end' | 'baseline' | 'stretch';
  justify?: 'start' | 'center' | 'end' | 'between';
  wrap?: boolean;
  /** Fill the remaining space of the parent stack. */
  grow?: boolean;
  pad?: Space;
  /** Scroll on overflow (and allow shrinking inside a grid/flex parent). */
  scroll?: boolean;
  as?: 'div' | 'form' | 'ul' | 'li' | 'nav' | 'section' | 'header';
  onSubmit?: () => void;
  role?: string;
  id?: string;
  'aria-label'?: string;
  'aria-labelledby'?: string;
  'aria-live'?: 'polite' | 'off';
  'data-testid'?: string;
}) {
  const t = useTheme();
  const map = {
    start: 'flex-start',
    center: 'center',
    end: 'flex-end',
    baseline: 'baseline',
    stretch: 'stretch',
    between: 'space-between',
  } as const;
  const style: CSSProperties = {
    display: 'flex',
    flexDirection: dir === 'row' ? 'row' : 'column',
    gap: t.space[gap],
    ...(align ? { alignItems: map[align] } : null),
    ...(justify ? { justifyContent: map[justify] } : null),
    ...(wrap ? { flexWrap: 'wrap' } : null),
    ...(grow ? { flex: 1, minWidth: 0, minHeight: 0 } : null),
    ...(pad !== undefined ? { padding: t.space[pad] } : null),
    ...(scroll ? { overflow: 'auto', minHeight: 0 } : null),
    ...(Tag === 'ul'
      ? {
          listStyle: 'none',
          margin: 0,
          padding: pad !== undefined ? t.space[pad] : 0,
        }
      : null),
  };
  return (
    <Tag
      style={style}
      role={role}
      id={id}
      aria-label={ariaLabel}
      aria-labelledby={labelledBy}
      aria-live={ariaLive}
      data-testid={testId}
      {...(Tag === 'form'
        ? {
            onSubmit: (e: FormEvent) => {
              e.preventDefault();
              onSubmit?.();
            },
          }
        : null)}
    >
      {children}
    </Tag>
  );
}

/** Whole-window app frame: content fills the window, tiny tabs along the bottom edge. */
export function AppFrame({
  children,
  tabs,
  embedded,
}: {
  children: ReactNode;
  tabs: ReactNode;
  /** Preview inside another page (gallery): no window drag strip, no main landmark. */
  embedded?: boolean;
}) {
  const t = useTheme();
  const Main = embedded ? 'div' : 'main';
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateRows: '1fr auto',
        height: '100%',
        background: t.color.bg,
      }}
    >
      {embedded ? null : <div className="q-drag" aria-hidden />}
      <Main style={{ minHeight: 0, overflow: 'auto' }}>{children}</Main>
      {tabs}
    </div>
  );
}

/** Minimal bottom tab bar (lowercase labels). Each tab gets data-testid `nav-<id>`. */
export function TabBar({
  items,
  active,
  onSelect,
  label = 'Apps',
  testIdPrefix = 'nav',
}: {
  items: { id: string; label: string }[];
  active: string;
  onSelect: (id: string) => void;
  label?: string;
  testIdPrefix?: string;
}) {
  const t = useTheme();
  return (
    <nav
      aria-label={label}
      style={{
        display: 'flex',
        justifyContent: 'center',
        gap: t.space[1],
        padding: `${t.space[1]} ${t.space[3]}`,
        borderTop: `1px solid ${t.color.border}`,
      }}
    >
      {items.map((it) => {
        const on = it.id === active;
        return (
          <button
            key={it.id}
            type="button"
            data-testid={`${testIdPrefix}-${it.id}`}
            aria-current={on ? 'page' : undefined}
            onClick={() => onSelect(it.id)}
            style={{
              all: 'unset',
              cursor: 'pointer',
              padding: `2px ${t.space[3]}`,
              borderRadius: t.radius.sm,
              background: on ? t.color.accent : 'transparent',
              transition: transition('background', 'hover', t),
            }}
          >
            <Text variant="caption" tone={on ? 'default' : 'muted'}>
              {it.label.toLowerCase()}
            </Text>
          </button>
        );
      })}
    </nav>
  );
}

/** Two-pane layout: fixed-width sidebar (list) and a detail pane that fills the rest. */
export function SplitView({
  sidebar,
  children,
  'data-testid': testId,
}: {
  sidebar: ReactNode;
  children: ReactNode;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div
      data-testid={testId}
      style={{
        display: 'grid',
        gridTemplateColumns: '264px 1fr',
        height: '100%',
        minHeight: 0,
      }}
    >
      <aside
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: t.space[2],
          padding: `40px ${t.space[3]} ${t.space[3]}`,
          borderRight: `1px solid ${t.color.border}`,
          minHeight: 0,
          overflow: 'hidden',
        }}
      >
        {sidebar}
      </aside>
      <section
        style={{
          minWidth: 0,
          minHeight: 0,
          overflow: 'auto',
          padding: `40px ${t.space[8]} ${t.space[8]}`,
        }}
      >
        {children}
      </section>
    </div>
  );
}

/** Single centered column page (Settings, Gallery). */
export function Page({
  children,
  width = 640,
  'data-testid': testId,
}: {
  children: ReactNode;
  width?: number;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div
      data-testid={testId}
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: t.space[6],
        maxWidth: width,
        margin: '0 auto',
        padding: `40px ${t.space[6]} ${t.space[8]}`,
      }}
    >
      {children}
    </div>
  );
}

/** A horizontal row of controls; `end` pushes trailing children to the right. */
export function Toolbar({
  children,
  end,
  'data-testid': testId,
}: {
  children?: ReactNode;
  end?: ReactNode;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div
      data-testid={testId}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: t.space[2],
        minHeight: 28,
      }}
    >
      {children}
      {end ? (
        <div
          style={{
            marginLeft: 'auto',
            display: 'flex',
            alignItems: 'center',
            gap: t.space[2],
          }}
        >
          {end}
        </div>
      ) : null}
    </div>
  );
}

/** Square icon button with an accessible label. */
export function IconButton({
  icon,
  label,
  onClick,
  pressed,
  tone = 'default',
  'data-testid': testId,
  'aria-expanded': expanded,
}: {
  icon: ReactNode;
  label: string;
  onClick: () => void;
  pressed?: boolean;
  tone?: 'default' | 'record';
  'data-testid'?: string;
  'aria-expanded'?: boolean;
}) {
  const t = useTheme();
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      aria-expanded={expanded}
      data-testid={testId}
      onClick={onClick}
      style={{
        all: 'unset',
        cursor: 'pointer',
        width: 26,
        height: 26,
        flexShrink: 0,
        display: 'grid',
        placeItems: 'center',
        borderRadius: t.radius.md,
        border: `1px solid ${t.color.border}`,
        background: pressed ? t.color.accent : 'transparent',
        color: tone === 'record' ? t.color.recording : t.color.text,
        ...typeStyle('caption', t),
        transition: transition('background', 'hover', t),
      }}
    >
      {icon}
    </button>
  );
}

/** Small toggleable chip (speakers, citations). */
export function Chip({
  children,
  onClick,
  pressed,
  title,
  'aria-label': ariaLabel,
  'data-testid': testId,
}: {
  children: ReactNode;
  onClick?: () => void;
  pressed?: boolean;
  title?: string;
  'aria-label'?: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={pressed}
      aria-label={ariaLabel}
      title={title}
      data-testid={testId}
      style={{
        all: 'unset',
        cursor: 'pointer',
        ...typeStyle('caption', t),
        color: t.color.textMuted,
        background: pressed ? t.color.accent : 'transparent',
        border: `1px solid ${t.color.border}`,
        borderRadius: t.radius.sm,
        padding: `0 ${t.space[2]}`,
      }}
    >
      {children}
    </button>
  );
}

/** A selectable list row: title, quiet meta line, optional snippet. Used for the meetings sidebar. */
export function ListRow({
  title,
  meta,
  snippet,
  selected,
  onSelect,
  'data-testid': testId,
}: {
  title: ReactNode;
  meta?: ReactNode;
  snippet?: ReactNode;
  selected?: boolean;
  onSelect: () => void;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <li style={{ listStyle: 'none' }}>
      <button
        type="button"
        data-testid={testId}
        aria-current={selected || undefined}
        onClick={onSelect}
        style={{
          all: 'unset',
          boxSizing: 'border-box',
          cursor: 'pointer',
          width: '100%',
          display: 'grid',
          gap: 2,
          padding: `${t.space[2]} ${t.space[3]}`,
          borderRadius: t.radius.md,
          background: selected ? t.color.accent : 'transparent',
          contentVisibility: 'auto',
          containIntrinsicSize: 'auto 44px',
          transition: transition('background', 'hover', t),
        }}
      >
        <Text truncate>{title}</Text>
        {meta ? (
          <Text variant="caption" muted truncate>
            {meta}
          </Text>
        ) : null}
        {snippet ? (
          <Text variant="caption" muted data-testid="search-snippet">
            {snippet}
          </Text>
        ) : null}
      </button>
    </li>
  );
}

/** Meeting row in the sidebar list (title + date). */
export function MeetingRow(p: {
  title: string;
  date: string;
  snippet?: string;
  selected?: boolean;
  onSelect: () => void;
}) {
  return (
    <ListRow
      title={p.title}
      meta={p.date}
      {...(p.snippet ? { snippet: p.snippet } : {})}
      {...(p.selected ? { selected: true } : {})}
      onSelect={p.onSelect}
      data-testid="meeting-item"
    />
  );
}

/** Settings group: a heading and its rows. */
export function SettingsSection({
  title,
  children,
  'data-testid': testId,
}: {
  title: string;
  children: ReactNode;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <section
      data-testid={testId}
      style={{ display: 'flex', flexDirection: 'column', gap: t.space[3] }}
    >
      <Text variant="label" tone="muted" style={{ textTransform: 'uppercase' }}>
        {title}
      </Text>
      {children}
    </section>
  );
}

/** One setting: label on the left, control on the right. */
export function SettingRow({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  const t = useTheme();
  // A <label> so Selects/Inputs get their accessible name and clicking the text toggles a Switch.
  return (
    <label
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: t.space[4],
        minHeight: 28,
      }}
    >
      <Text>{label}</Text>
      <div
        style={{
          flex: '0 0 auto',
          width: 240,
          display: 'flex',
          justifyContent: 'flex-end',
        }}
      >
        {children}
      </div>
    </label>
  );
}

/** Keyboard helper: Enter/Space activates (for custom interactive rows). */
export const activateOnKey = (fn: () => void) => (e: KeyboardEvent) => {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault();
    fn();
  }
};
