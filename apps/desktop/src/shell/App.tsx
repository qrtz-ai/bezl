import { lazy, Suspense, useMemo, useState } from 'react';
import { ThemeProvider, Text, useTheme } from '@qrtz/ui';
import { registry } from './subapps';

function Sidebar({
  active,
  onSelect,
}: {
  active: string;
  onSelect: (id: string) => void;
}) {
  const t = useTheme();
  return (
    <nav
      style={{
        width: 200,
        padding: `40px ${t.space[2]} ${t.space[2]}`,
        borderRight: `1px solid ${t.color.border}`,
        background: t.color.surface,
        display: 'grid',
        alignContent: 'start',
        gap: t.space[1],
      }}
    >
      {registry.list().map(({ manifest }) => (
        <button
          key={manifest.id}
          type="button"
          onClick={() => onSelect(manifest.id)}
          style={{
            all: 'unset',
            cursor: 'pointer',
            padding: `${t.space[2]} ${t.space[3]}`,
            borderRadius: t.radius.sm,
            background: manifest.id === active ? t.color.border : 'transparent',
          }}
        >
          <Text>
            {manifest.icon}&nbsp;&nbsp;{manifest.name}
          </Text>
        </button>
      ))}
    </nav>
  );
}

export function App() {
  const first = registry.list()[0]?.manifest.id ?? '';
  const [active, setActive] = useState(first);
  const View = useMemo(() => {
    const app = registry.get(active);
    return app ? lazy(app.load) : null;
  }, [active]);
  return (
    <ThemeProvider>
      <div style={{ display: 'flex', height: '100%' }}>
        <div className="drag-region" />
        <Sidebar active={active} onSelect={setActive} />
        <main style={{ flex: 1, padding: 32, overflow: 'auto' }}>
          <Suspense fallback={null}>{View ? <View /> : null}</Suspense>
        </main>
      </div>
    </ThemeProvider>
  );
}
