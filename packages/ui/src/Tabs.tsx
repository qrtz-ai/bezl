import * as TabsPrimitive from '@radix-ui/react-tabs';
import type { ReactNode } from 'react';
import { typeStyle } from './tokens';
import { useTheme } from './theme';

/**
 * A row of tabs on Radix Tabs (shadcn `Tabs`): ArrowLeft/ArrowRight/Home/End move between tabs (roving focus) and
 * select them. Only the list lives here; panels are separate (KeepAlive), so triggers carry no aria-controls.
 */
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
    <TabsPrimitive.Root value={value} onValueChange={(v) => onChange(v as T)}>
      <TabsPrimitive.List
        style={{
          display: 'flex',
          gap: t.space[4],
          borderBottom: `1px solid ${t.color.border}`,
        }}
      >
        {tabs.map((tab) => {
          const on = tab.id === value;
          return (
            <TabsPrimitive.Trigger
              key={tab.id}
              value={tab.id}
              aria-controls={undefined}
              data-testid={`tab-${tab.id}`}
              // Radix selects on mousedown, focus and keys; a synthetic click (assistive tech) selects too.
              onClick={() => onChange(tab.id)}
              style={{
                all: 'unset',
                cursor: 'pointer',
                padding: `${t.space[2]} 0`,
                marginBottom: -1,
                ...typeStyle('label', t),
                color: on ? t.color.text : t.color.textMuted,
                borderBottom: `2px solid ${on ? t.color.primary : 'transparent'}`,
                transition: `color ${t.motion.fast} ${t.motion.easeOut}`,
              }}
            >
              {tab.label.toLowerCase()}
            </TabsPrimitive.Trigger>
          );
        })}
      </TabsPrimitive.List>
    </TabsPrimitive.Root>
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
