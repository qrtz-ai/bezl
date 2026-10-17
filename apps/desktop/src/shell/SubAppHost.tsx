import {
  Component,
  lazy,
  Suspense,
  useEffect,
  type ComponentType,
  type LazyExoticComponent,
  type ReactNode,
} from 'react';
import { createClient, type QrtzClient, type SubApp } from '@qrtz/core';
import { Button, KeepAlive, Stack, Text } from '@qrtz/ui';
import { QrtzClientProvider } from '@qrtz/core/react';
import { noTransport } from './shellClient';

class SubAppErrorBoundary extends Component<
  { name: string; children: ReactNode },
  { error: Error | null }
> {
  override state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  override componentDidCatch(error: Error) {
    console.error(`[qrtz] sub app "${this.props.name}" crashed`, error);
  }
  override render() {
    if (!this.state.error) return this.props.children;
    return (
      <Stack
        role="alert"
        data-testid="subapp-error"
        gap={3}
        pad={8}
        align="start"
      >
        <Text variant="h3">{this.props.name} hit an error</Text>
        <Text muted>{this.state.error.message}</Text>
        <Button variant="ghost" onClick={() => this.setState({ error: null })}>
          retry
        </Button>
      </Stack>
    );
  }
}

// One lazy view and one client per sub app id for the life of the window: a fresh `lazy()` on every
// render would suspend (blank frame) and remount the whole sub app even when its module is cached.
const views = new Map<string, LazyExoticComponent<ComponentType>>();
const clients = new Map<string, QrtzClient>();
const viewFor = (app: SubApp) => {
  let v = views.get(app.manifest.id);
  if (!v) views.set(app.manifest.id, (v = lazy(app.load)));
  return v;
};
const clientFor = (app: SubApp) => {
  let c = clients.get(app.manifest.id);
  if (!c)
    clients.set(
      app.manifest.id,
      (c = createClient(window.qrtzTransport ?? noTransport, app.manifest.id)),
    );
  return c;
};

/**
 * Mounts one sub app with its own scoped IPC client, error boundary and lifecycle. The shell keeps a
 * visited sub app mounted while another is shown: `active={false}` hides it (`hidden`, `inert`,
 * `aria-hidden`) and runs its `activate()` cleanup, since activate is per visible session.
 */
export function SubAppHost({
  app,
  active = true,
}: {
  app: SubApp;
  active?: boolean;
}) {
  const View = viewFor(app);
  const client = clientFor(app);
  useEffect(() => {
    if (!active) return;
    const cleanup = app.activate?.();
    return () => {
      if (typeof cleanup === 'function') cleanup();
    };
  }, [app, active]);
  return (
    <KeepAlive active={active} fill data-subapp={app.manifest.id}>
      <QrtzClientProvider client={client}>
        <SubAppErrorBoundary key={app.manifest.id} name={app.manifest.name}>
          <Suspense fallback={null}>
            <View />
          </Suspense>
        </SubAppErrorBoundary>
      </QrtzClientProvider>
    </KeepAlive>
  );
}
