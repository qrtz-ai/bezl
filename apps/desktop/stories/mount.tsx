import { createMemoryRouter, RouterProvider } from 'react-router';
import { createTauriTransport } from '@qrtz/core';
import { createMockBridge } from '../src/mock/backend';

type Routes = Parameters<typeof createMemoryRouter>[0];
let routes: Promise<Routes> | undefined;

/**
 * The shell's routes behind the in-page mock backend, as `bun run dev:mock` runs them. `?dev=1` lists the
 * dev-only sub apps (dash) the same way unpackaged builds do; set before the registry is first imported.
 */
export function loadRoutes(): Promise<Routes> {
  routes ??= (async () => {
    const q = new URLSearchParams(location.search);
    if (q.get('dev') !== '1') {
      q.set('dev', '1');
      history.replaceState(
        null,
        '',
        `${location.pathname}?${q}${location.hash}`,
      );
    }
    window.qrtzTransport = createTauriTransport(createMockBridge());
    return (await import('../src/shell/App')).routes;
  })();
  return routes;
}

export function Page({ routes: r, path }: { routes: Routes; path: string }) {
  return (
    <RouterProvider
      key={path}
      router={createMemoryRouter(r, { initialEntries: [path] })}
    />
  );
}
