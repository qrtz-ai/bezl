import { useCallback, useEffect, useState } from 'react';
import { Kbd, ThemeProvider, Text, ToastProvider, useTheme } from '@qrtz/ui';
import { Palette } from './Palette';
import { onNavigateRequest } from '@qrtz/core';
import { registry } from './subapps';
import { SubAppHost } from './SubAppHost';

function Sidebar({
  active,
  onSelect,
  onPalette,
}: {
  active: string;
  onSelect: (id: string) => void;
  onPalette: () => void;
}) {
  const t = useTheme();
  return (
    <nav
      style={{
        width: 216,
        padding: `44px ${t.space[3]} ${t.space[3]}`,
        borderRight: `1px solid ${t.color.border}`,
        background: t.color.surface,
        display: 'flex',
        flexDirection: 'column',
        gap: t.space[1],
      }}
    >
      <div
        style={{
          padding: `0 ${t.space[3]} ${t.space[4]}`,
          display: 'flex',
          alignItems: 'baseline',
          gap: 2,
        }}
      >
        <Text mono weight={600} size="lg" data-testid="brand">
          qrtz
        </Text>
        <span
          aria-hidden
          style={{
            width: 8,
            height: 16,
            background: t.color.text,
            animation: 'q-blink 1.1s step-end infinite',
            alignSelf: 'center',
          }}
        />
      </div>
      <button
        type="button"
        data-testid="open-palette"
        onClick={onPalette}
        style={{
          all: 'unset',
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: `${t.space[2]} ${t.space[3]}`,
          marginBottom: t.space[3],
          border: `1px solid ${t.color.border}`,
          borderRadius: t.radius.md,
          background: t.color.bg,
        }}
      >
        <Text size="sm" muted>
          Search…
        </Text>
        <span>
          <Kbd>⌘</Kbd> <Kbd>K</Kbd>
        </span>
      </button>
      {registry.list().map(({ manifest }) => {
        const on = manifest.id === active;
        return (
          <button
            key={manifest.id}
            data-testid={`nav-${manifest.id}`}
            type="button"
            aria-current={on ? 'page' : undefined}
            onClick={() => onSelect(manifest.id)}
            className="q-nav"
            style={{
              all: 'unset',
              cursor: 'pointer',
              display: 'flex',
              alignItems: 'center',
              gap: t.space[3],
              padding: `${t.space[2]} ${t.space[3]}`,
              borderRadius: t.radius.md,
              background: on ? t.color.accent : 'transparent',
              color: on ? t.color.text : t.color.textMuted,
              transition: `background ${t.motion.fast}, color ${t.motion.fast}`,
            }}
          >
            <span style={{ width: 16, textAlign: 'center', fontSize: 13 }}>
              {manifest.icon}
            </span>
            <Text
              size="sm"
              style={{ color: 'inherit' }}
              weight={on ? 500 : 400}
            >
              {manifest.name}
            </Text>
          </button>
        );
      })}
      <div style={{ flex: 1 }} />
      <Text size="xs" mono muted style={{ padding: `0 ${t.space[3]}` }}>
        ⌘⇧R record
      </Text>
    </nav>
  );
}

export function App() {
  const first = registry.list()[0]?.manifest.id ?? '';
  const [active, setActive] = useState(first);
  const [palette, setPalette] = useState(false);
  const closePalette = useCallback(() => setPalette(false), []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setPalette((p) => !p);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
  useEffect(() => onNavigateRequest((r) => setActive(r.app)), []);
  const app = registry.get(active);
  return (
    <ThemeProvider>
      <ToastProvider>
        <Palette open={palette} onClose={closePalette} onNavigate={setActive} />
        <div style={{ display: 'flex', height: '100%' }}>
          <div className="drag-region" />
          <Sidebar
            active={active}
            onSelect={setActive}
            onPalette={() => setPalette(true)}
          />
          <main style={{ flex: 1, padding: 32, overflow: 'auto' }}>
            {app ? <SubAppHost app={app} /> : null}
          </main>
        </div>
      </ToastProvider>
    </ThemeProvider>
  );
}
