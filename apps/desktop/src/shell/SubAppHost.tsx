import {
  Component,
  Suspense,
  useEffect,
  useMemo,
  type ComponentType,
  type ReactNode,
} from 'react';
import { createClient, type QrtzClient, type SubApp } from '@qrtz/core';
import { Button, KeepAlive, PaneLoading, Stack, Text } from '@qrtz/ui';
import {
  QrtzClientProvider,
  SubAppRouteProvider,
  type SubAppRoute,
} from '@qrtz/core/react';
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

// One module load and one client per sub app id for the life of the window: a fresh load on every render
// would suspend (blank frame) and remount the whole sub app even when its module is cached.
type Load =
  | { status: 'pending'; promise: Promise<void> }
  | { status: 'done'; View: ComponentType }
  | { status: 'error'; error: unknown };
const loads = new Map<string, Load>();
const clients = new Map<string, QrtzClient>();
/** The sub app's view, or suspends while its module loads (on the shell's boundary, see `ViewGate`). */
const viewFor = (app: SubApp): ComponentType => {
  const id = app.manifest.id;
  let l = loads.get(id);
  if (!l) {
    const promise = app.load().then(
      (m) => void loads.set(id, { status: 'done', View: m.default }),
      (error: unknown) => void loads.set(id, { status: 'error', error }),
    );
    loads.set(id, (l = { status: 'pending', promise }));
  }
  if (l.status === 'pending') throw l.promise;
  if (l.status === 'error') {
    loads.delete(id); // the error boundary's retry loads it again
    throw l.error;
  }
  return l.View;
};

/**
 * The module load suspends here, above this sub app's own boundary, so a first visit suspends the shell's
 * boundary inside the router's transition, and the previous app stays on screen. Once loaded, anything the view
 * suspends on is caught by its own boundary: a hidden app can never blank the visible one. That boundary shows
 * `PaneLoading`, never nothing: a first visit whose view suspends right after its module loaded (a lazy screen,
 * data) shows the spinner, not an empty frame. A suspend inside a transition keeps the shown content, as usual.
 */
function ViewGate({ app }: { app: SubApp }) {
  const View = viewFor(app);
  return (
    <Suspense fallback={<PaneLoading />}>
      <View />
    </Suspense>
  );
}
const noNavigate: SubAppRoute['navigate'] = () => {};
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
 * `aria-hidden`) and runs its `activate()` cleanup, since activate is per visible session. `path` and
 * `navigate` are its slice of the shell's route (ADR 0091). The shell's boundary covers only the first module
 * load; each sub app has its own boundary for what it suspends on after that.
 */
export function SubAppHost({
  app,
  active = true,
  path = '',
  navigate = noNavigate,
}: {
  app: SubApp;
  active?: boolean;
  path?: string;
  navigate?: SubAppRoute['navigate'];
}) {
  const client = clientFor(app);
  const route = useMemo(
    () => ({ active, path, navigate }),
    [active, path, navigate],
  );
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
        <SubAppRouteProvider route={route}>
          <SubAppErrorBoundary key={app.manifest.id} name={app.manifest.name}>
            <ViewGate app={app} />
          </SubAppErrorBoundary>
        </SubAppRouteProvider>
      </QrtzClientProvider>
    </KeepAlive>
  );
}
