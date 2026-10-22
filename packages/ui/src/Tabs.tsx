import { Tabs as TabsRoot, TabsList, TabsTrigger } from './components/ui/tabs';
import type { ReactNode } from 'react';

/**
 * A row of underline tabs on shadcn's Tabs (`variant="line"`): ArrowLeft/ArrowRight/Home/End move between tabs
 * (roving focus) and select them. Only the list lives here; panels are separate (KeepAlive), so triggers carry
 * no aria-controls.
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
  return (
    <TabsRoot value={value} onValueChange={(v) => onChange(v as T)}>
      <TabsList variant="line">
        {tabs.map((tab) => (
          <TabsTrigger
            key={tab.id}
            value={tab.id}
            aria-controls={undefined}
            data-testid={`tab-${tab.id}`}
            // Radix selects on mousedown, focus and keys; a synthetic click (assistive tech) selects too.
            onClick={() => onChange(tab.id)}
          >
            {tab.label.toLowerCase()}
          </TabsTrigger>
        ))}
      </TabsList>
    </TabsRoot>
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
