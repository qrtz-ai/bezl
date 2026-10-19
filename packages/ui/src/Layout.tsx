import {
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type Ref,
} from 'react';
import { useTheme } from './theme';
import { Text } from './Text';
import { Heading } from './Heading';
import { Icon, isIconName, type IconName } from './Icon';
import { transition, typeStyle, type Tokens } from './tokens';
import { lowerTitle } from './case';

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
  'data-live': dataLive,
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
  /** Marks a live (recording) view for tests and assistive styling. */
  'data-live'?: boolean;
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
      data-live={dataLive ? '' : undefined}
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

/** Whole-window app frame: the nav sidebar on the left, content fills the rest. There is no bottom bar. */
export function AppFrame({
  children,
  sidebar,
  embedded,
}: {
  children: ReactNode;
  sidebar: ReactNode;
  /** Preview inside another page (gallery): no window drag strip, no main landmark. */
  embedded?: boolean;
}) {
  const t = useTheme();
  const Main = embedded ? 'div' : 'main';
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: 'auto minmax(0, 1fr)',
        height: '100%',
        background: t.color.bg,
        overflow: 'hidden',
      }}
    >
      {embedded ? null : <div className="q-drag" aria-hidden />}
      {sidebar}
      <Main
        data-scroll-root=""
        style={{
          minWidth: 0,
          minHeight: 0,
          overflowX: 'hidden',
          overflowY: 'auto',
          overscrollBehavior: 'none',
          scrollbarGutter: 'stable',
        }}
      >
        {children}
      </Main>
    </div>
  );
}

export type SideNavItem = {
  id: string;
  label: string;
  /** Feather icon name; an unknown name renders the label alone. */
  icon?: string;
  /** `foot` items sit pinned at the bottom of the sidebar (settings, gallery). */
  group?: 'main' | 'foot';
};

/** The app's main navigation: a vertical sidebar of icon + lowercase label rows. Each row gets data-testid `nav-<id>`. */
export function SideNav({
  items,
  active,
  onSelect,
  label = 'Apps',
  testIdPrefix = 'nav',
  embedded,
}: {
  items: SideNavItem[];
  active: string;
  onSelect: (id: string) => void;
  label?: string;
  testIdPrefix?: string;
  /** Inside a preview frame: no room left for the window controls. */
  embedded?: boolean;
}) {
  const t = useTheme();
  const row = (it: SideNavItem) => {
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
          boxSizing: 'border-box',
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          gap: t.space[2],
          width: '100%',
          padding: `${t.space[1]} ${t.space[2]}`,
          borderRadius: t.radius.md,
          background: on ? t.color.accent : 'transparent',
          color: on ? t.color.text : t.color.textMuted,
          transition: transition('background', 'hover', t),
        }}
      >
        {isIconName(it.icon) ? <Icon name={it.icon} size="sm" /> : null}
        <Text tone={on ? 'default' : 'muted'} truncate>
          {it.label.toLowerCase()}
        </Text>
      </button>
    );
  };
  const main = items.filter((i) => i.group !== 'foot');
  const foot = items.filter((i) => i.group === 'foot');
  return (
    <nav
      aria-label={label}
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 2,
        width: 176,
        boxSizing: 'border-box',
        padding: `${embedded ? t.space[2] : '40px'} ${t.space[2]} ${t.space[2]}`,
        borderRight: `1px solid ${t.color.border}`,
        minHeight: 0,
        overflow: 'auto',
      }}
    >
      {main.map(row)}
      {foot.length ? (
        <div
          style={{
            marginTop: 'auto',
            display: 'flex',
            flexDirection: 'column',
            gap: 2,
            paddingTop: t.space[2],
          }}
        >
          {foot.map(row)}
        </div>
      ) : null}
    </nav>
  );
}

/**
 * The one page geometry every routed view shares, so nothing moves when you switch pages in the sidebar:
 * content column max 880px, centered in the main area; 40px top (level with the sidebar's first row), 32px
 * gutters, 48px bottom; a 40px header row (title left, actions right); 24px between sections. Pages never
 * scroll sideways (the AppFrame main scrolls vertically only and keeps a stable scrollbar gutter).
 */
export const PAGE = {
  maxWidth: 880,
  padTop: 40,
  gutter: 32,
  padBottom: 48,
  headerHeight: 40,
  gap: 24,
} as const;

/**
 * The shared page container. `title` is the page's h1 (lowercased; `preserveCase` for user content), `leading`
 * sits before it (a back button), `actions` on the right of the header row. Children are the page's sections.
 * Marks itself `data-page` (and its header `data-page-header`) so tests can check every route uses it.
 */
export function Page({
  children,
  title,
  leading,
  actions,
  preserveCase,
  inset,
  'data-testid': testId,
}: {
  children?: ReactNode;
  /** A page shown inside another page's column (nano's settings in its detail pane): same header row and
   * section spacing, no outer padding or width of its own. */
  inset?: boolean;
  title: ReactNode;
  leading?: ReactNode;
  actions?: ReactNode;
  preserveCase?: boolean;
  'data-testid'?: string;
}) {
  return (
    <div
      data-testid={testId}
      {...(inset ? { 'data-page-inset': '' } : { 'data-page': '' })}
      style={{
        boxSizing: 'border-box',
        display: 'flex',
        flexDirection: 'column',
        gap: PAGE.gap,
        width: '100%',
        maxWidth: inset ? '100%' : PAGE.maxWidth + 2 * PAGE.gutter,
        minWidth: 0,
        margin: inset ? 0 : '0 auto',
        padding: inset
          ? 0
          : `${PAGE.padTop}px ${PAGE.gutter}px ${PAGE.padBottom}px`,
        overflowX: 'hidden',
      }}
    >
      <header
        data-page-header=""
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          height: PAGE.headerHeight,
          minHeight: PAGE.headerHeight,
          flex: '0 0 auto',
          minWidth: 0,
        }}
      >
        {leading}
        <Heading
          {...(preserveCase ? { preserveCase } : {})}
          style={{
            minWidth: 0,
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {title}
        </Heading>
        {actions ? (
          <div
            style={{
              marginLeft: 'auto',
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              minWidth: 0,
              flex: '0 1 auto',
            }}
          >
            {actions}
          </div>
        ) : null}
      </header>
      {children}
    </div>
  );
}

/** Two columns inside a Page body: a fixed 240px list and a detail column that takes the rest (nano). */
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
        gridTemplateColumns: `240px minmax(0, 1fr)`,
        gap: PAGE.gutter,
        minWidth: 0,
        alignItems: 'start',
      }}
    >
      <aside
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: t.space[2],
          minWidth: 0,
          minHeight: 0,
        }}
      >
        {sidebar}
      </aside>
      <section style={{ minWidth: 0, minHeight: 0 }}>{children}</section>
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

/**
 * Square icon button with an accessible label (also its tooltip). `badge` draws a small dot on its corner (an
 * update is waiting); `onClick` gets the event, so a button inside a clickable row can stop it.
 */
export function IconButton({
  icon,
  label,
  onClick,
  pressed,
  disabled,
  badge,
  tone = 'default',
  ref,
  'data-testid': testId,
  'aria-expanded': expanded,
  'aria-haspopup': hasPopup,
  'aria-controls': controls,
}: {
  icon: IconName;
  label: string;
  onClick: (e: MouseEvent<HTMLButtonElement>) => void;
  pressed?: boolean;
  disabled?: boolean;
  badge?: boolean;
  tone?: 'default' | 'record' | 'danger';
  ref?: Ref<HTMLButtonElement>;
  'data-testid'?: string;
  'aria-expanded'?: boolean;
  'aria-haspopup'?: 'menu';
  'aria-controls'?: string;
}) {
  const t = useTheme();
  return (
    <button
      ref={ref}
      type="button"
      aria-label={label.toLowerCase()}
      title={label.toLowerCase()}
      aria-pressed={pressed}
      aria-expanded={expanded}
      aria-haspopup={hasPopup}
      aria-controls={controls}
      disabled={disabled}
      data-testid={testId}
      data-badge={badge ? '' : undefined}
      onClick={onClick}
      style={{
        all: 'unset',
        position: 'relative',
        boxSizing: 'border-box',
        cursor: disabled ? 'not-allowed' : 'pointer',
        opacity: disabled ? 0.5 : 1,
        width: 26,
        height: 26,
        flexShrink: 0,
        display: 'grid',
        placeItems: 'center',
        borderRadius: t.radius.md,
        border: `1px solid ${tone === 'danger' ? t.color.danger : t.color.border}`,
        background: pressed ? t.color.accent : 'transparent',
        color:
          tone === 'record'
            ? t.color.recording
            : tone === 'danger'
              ? t.color.danger
              : t.color.text,
        ...typeStyle('caption', t),
        transition: transition('background', 'hover', t),
      }}
    >
      <Icon name={icon} size="sm" fill={icon === 'stop'} />
      {badge ? (
        <span
          aria-hidden
          data-testid={testId ? `${testId}-badge` : undefined}
          style={{
            position: 'absolute',
            top: -3,
            right: -3,
            width: 8,
            height: 8,
            borderRadius: 4,
            background: t.color.warning,
            border: `2px solid ${t.color.bg}`,
          }}
        />
      ) : null}
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
      <Text variant="label" tone="muted">
        {lowerTitle(title)}
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
      <Text style={{ minWidth: 0, flex: '1 1 auto' }}>{lowerTitle(label)}</Text>
      <div
        style={{
          flex: '0 1 240px',
          minWidth: 0,
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
