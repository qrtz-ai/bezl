import { startTransition, useCallback, useEffect, useState } from 'react';
import { AppFrame, TabBar, ThemeProvider, ToastProvider } from '@qrtz/ui';
import { Palette } from './Palette';
import { onNavigateRequest } from '@qrtz/core';
import { registry } from './subapps';
import { SubAppHost } from './SubAppHost';
import { useConfigTheme } from './useConfigTheme';
import { Onboarding } from './Onboarding';
import { useOnboarded } from './useOnboarded';

export function App() {
  const first = registry.list()[0]?.manifest.id ?? '';
  const [active, setActiveNow] = useState(first);
  // Sub apps mount on first visit and then stay mounted (hidden) so switching back is instant.
  const [visited, setVisited] = useState<ReadonlySet<string>>(
    () => new Set([first]),
  );
  // A transition keeps the current tab on screen while a first visit's lazy module loads.
  const setActive = useCallback((id: string) => {
    startTransition(() => {
      setActiveNow(id);
      setVisited((v) => (v.has(id) ? v : new Set(v).add(id)));
    });
  }, []);
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
  useEffect(() => onNavigateRequest((r) => setActive(r.app)), [setActive]);
  const theme = useConfigTheme();
  const [onboarded, setOnboarded] = useOnboarded();
  if (onboarded === false)
    return (
      <ThemeProvider mode={theme}>
        <ToastProvider>
          <Onboarding onDone={() => setOnboarded(true)} />
        </ToastProvider>
      </ThemeProvider>
    );
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
          {registry
            .list()
            .filter((a) => visited.has(a.manifest.id))
            .map((a) => (
              <SubAppHost
                key={a.manifest.id}
                app={a}
                active={a.manifest.id === active}
              />
            ))}
        </AppFrame>
      </ToastProvider>
    </ThemeProvider>
  );
}
