import type { ReactNode } from 'react';
import { typeStyle, tokens } from './tokens';
import { useTheme } from './theme';

export function Tabs<T extends string>({
  tabs,
  value,
  onChange,
}: {
  tabs: { id: T; label: string }[];
  value: T;
  onChange: (id: T) => void;
}) {
  const t = useTheme();
  return (
    <div
      role="tablist"
      style={{
        display: 'flex',
        gap: t.space[4],
        borderBottom: `${tokens.border.hairline} solid ${t.color.border}`,
      }}
    >
      {tabs.map((tab) => {
        const on = tab.id === value;
        return (
          <button
            key={tab.id}
            role="tab"
            type="button"
            aria-selected={on}
            data-testid={`tab-${tab.id}`}
            onClick={() => onChange(tab.id)}
            style={{
              all: 'unset',
              cursor: 'pointer',
              padding: `${t.space[2]} 0`,
              marginBottom: `-${tokens.border.hairline}`,
              ...typeStyle('label', t),
              color: on ? t.color.text : t.color.textMuted,
              borderBottom: `${tokens.border.thick} solid ${on ? t.color.primary : 'transparent'}`,
              transition: `color ${t.motion.fast} ${t.motion.easeOut}`,
            }}
          >
            {tab.label.toLowerCase()}
          </button>
        );
      })}
    </div>
  );
}

/**
 * A view that stays mounted while another one is shown (tab panels, the shell's sub apps), so switching
 * back never remounts, refetches or flashes. Inactive: `hidden`, `inert` (no focus, clicks or the
 * Escape handlers of sheets inside it) and `aria-hidden`.
 */
export function KeepAlive({
  active,
  fill,
  role,
  children,
  ...data
}: {
  active: boolean;
  /** Take the parent's full height (a sub app). */
  fill?: boolean;
  role?: 'tabpanel';
  children: ReactNode;
} & { [k: `data-${string}`]: string }) {
  return (
    <div
      {...data}
      role={role}
      hidden={!active}
      inert={!active}
      aria-hidden={active ? undefined : true}
      style={
        active ? (fill ? { height: '100%' } : undefined) : { display: 'none' }
      }
    >
      {children}
    </div>
  );
}
