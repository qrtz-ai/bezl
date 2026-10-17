import { useCallback, useEffect, useState } from 'react';
import { AppFrame, TabBar, ThemeProvider, ToastProvider } from '@qrtz/ui';
import { Palette } from './Palette';
import { onNavigateRequest } from '@qrtz/core';
import { registry } from './subapps';
import { SubAppHost } from './SubAppHost';
import { useConfigTheme } from './useConfigTheme';

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
  const theme = useConfigTheme();
  const app = registry.get(active);
  return (
    <ThemeProvider mode={theme}>
      <ToastProvider>
        <Palette open={palette} onClose={closePalette} onNavigate={setActive} />
        <AppFrame
          tabs={
            <TabBar
              items={registry.list().map(({ manifest }) => ({
                id: manifest.id,
                label: manifest.name,
              }))}
              active={active}
              onSelect={setActive}
            />
          }
        >
          {app ? <SubAppHost app={app} /> : null}
        </AppFrame>
      </ToastProvider>
    </ThemeProvider>
  );
}
