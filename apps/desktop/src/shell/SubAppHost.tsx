import {
  Component,
  lazy,
  Suspense,
  useEffect,
  useMemo,
  type ReactNode,
} from 'react';
import { createClient, type QrtzTransport, type SubApp } from '@qrtz/core';
import { Button, Stack, Text } from '@qrtz/ui';
import { QrtzClientProvider } from '@qrtz/core/react';

export const noTransport: QrtzTransport = {
  invoke: async (e) => ({
    ok: false,
    id: e.id,
    error: {
      name: 'AppError',
      code: 'E_UNAVAILABLE',
      message: 'Not running inside qrtz',
      retryable: false,
    },
  }),
  on: () => () => {},
};

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

/** Mounts one sub app with its own scoped IPC client, error boundary and lifecycle. */
export function SubAppHost({ app }: { app: SubApp }) {
  const View = useMemo(() => lazy(app.load), [app]);
  const client = useMemo(
    () => createClient(window.qrtzTransport ?? noTransport, app.manifest.id),
    [app],
  );
  useEffect(() => {
    const cleanup = app.activate?.();
    return () => {
      if (typeof cleanup === 'function') cleanup();
    };
  }, [app]);
  return (
    <QrtzClientProvider client={client}>
      <SubAppErrorBoundary key={app.manifest.id} name={app.manifest.name}>
        <Suspense fallback={null}>
          <View />
        </Suspense>
      </SubAppErrorBoundary>
    </QrtzClientProvider>
  );
}
