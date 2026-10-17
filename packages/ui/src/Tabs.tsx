import { typeStyle } from './tokens';
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
        borderBottom: `1px solid ${t.color.border}`,
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
              marginBottom: -1,
              ...typeStyle('label', t),
              color: on ? t.color.text : t.color.textMuted,
              borderBottom: `2px solid ${on ? t.color.primary : 'transparent'}`,
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
