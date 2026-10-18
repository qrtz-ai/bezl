import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  createHashRouter,
  RouterProvider,
  useLocation,
  useNavigate,
  type RouteObject,
} from 'react-router';
import { AppFrame, TabBar, ThemeProvider, ToastProvider } from '@qrtz/ui';
import { onNavigateRequest } from '@qrtz/core';
import type { SubAppRoute } from '@qrtz/core/react';
import { registry } from './subapps';
import { SubAppHost } from './SubAppHost';
import { useConfigTheme } from './useConfigTheme';
import { useOnboarded } from './useOnboarded';
import { joinRoute, navTarget, resolveRoute, splitRoute } from './routes';

// Not needed to paint the shell (ADR 0093): onboarding shows once, the palette on ⌘K (preloaded when idle).
const loadPalette = () => import('./Palette');
const Palette = lazy(() => loadPalette().then((m) => ({ default: m.Palette })));
const Onboarding = lazy(() =>
  import('./Onboarding').then((m) => ({ default: m.Onboarding })),
);

/** One splat route: the shell owns the whole path and stays mounted across every navigation (ADR 0091). */
export const routes: RouteObject[] = [{ path: '*', element: <Shell /> }];

let router: ReturnType<typeof createHashRouter> | undefined;

export function App() {
  router ??= createHashRouter(routes);
  return <RouterProvider router={router} />;
}

export function Shell() {
  const apps = useMemo(() => registry.list().map((a) => a.manifest.id), []);
  const location = useLocation();
  const navigate = useNavigate();
  const path = resolveRoute(location.pathname, apps);
  const { app: active, sub } = splitRoute(path);
  // An unknown or malformed route is replaced by the canonical one (the views already render it).
  useEffect(() => {
    if (path !== location.pathname) void navigate(path, { replace: true });
  }, [path, location.pathname, navigate]);

  // Sub apps mount on first visit and then stay mounted (hidden) so switching back is instant; each keeps
  // the path it was last on, which its tab returns to.
  const [visited, setVisited] = useState<ReadonlySet<string>>(
    () => new Set([active]),
  );
  const [paths, setPaths] = useState<Readonly<Record<string, string>>>({});
  useEffect(() => {
    setVisited((v) => (v.has(active) ? v : new Set(v).add(active)));
    setPaths((p) => (p[active] === sub ? p : { ...p, [active]: sub }));
  }, [active, sub]);
  const pathsRef = useRef(paths);
  pathsRef.current = paths;
  const activeRef = useRef(active);
  activeRef.current = active;

  const selectApp = useCallback(
    (id: string) => void navigate(joinRoute(id, pathsRef.current[id])),
    [navigate],
  );
  // Stable per app: sub apps put navigate in effect deps.
  const navFns = useRef(new Map<string, SubAppRoute['navigate']>());
  const navFor = (id: string) => {
    let f = navFns.current.get(id);
    if (!f) {
      f = (to, opts) => {
        if (activeRef.current === id) void navigate(joinRoute(id, to), opts);
        else setPaths((p) => ({ ...p, [id]: to }));
      };
      navFns.current.set(id, f);
    }
    return f;
  };

  const [palette, setPalette] = useState(false);
  // Mounted from the first open on, so closing and reopening is instant.
  const [paletteUsed, setPaletteUsed] = useState(false);
  if (palette && !paletteUsed) setPaletteUsed(true);
  const closePalette = useCallback(() => setPalette(false), []);
  useEffect(() => {
    const h = setTimeout(() => void loadPalette(), 1500);
    return () => clearTimeout(h);
  }, []);
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
  useEffect(
    () => onNavigateRequest((r) => void navigate(navTarget(r, apps))),
    [navigate, apps],
  );
  const theme = useConfigTheme();
  const [onboarded, setOnboarded] = useOnboarded();
  if (onboarded === false)
    return (
      <ThemeProvider mode={theme}>
        <ToastProvider>
          <Suspense fallback={null}>
            <Onboarding onDone={() => setOnboarded(true)} />
          </Suspense>
        </ToastProvider>
      </ThemeProvider>
    );
  return (
    <ThemeProvider mode={theme}>
      <ToastProvider>
        {paletteUsed ? (
          <Suspense fallback={null}>
            <Palette
              open={palette}
              onClose={closePalette}
              onNavigate={selectApp}
            />
          </Suspense>
        ) : null}
        <AppFrame
          tabs={
            <TabBar
              items={registry.list().map(({ manifest }) => ({
                id: manifest.id,
                label: manifest.name,
              }))}
              active={active}
              onSelect={selectApp}
            />
          }
        >
          {/* One boundary that is already on screen: a first visit's lazy load suspends inside the router's
              transition, so the previous app stays up instead of a blank frame. */}
          <Suspense fallback={null}>
            {registry
              .list()
              .filter(
                (a) => a.manifest.id === active || visited.has(a.manifest.id),
              )
              .map((a) => {
                const id = a.manifest.id;
                return (
                  <SubAppHost
                    key={id}
                    app={a}
                    active={id === active}
                    path={id === active ? sub : (paths[id] ?? '')}
                    navigate={navFor(id)}
                  />
                );
              })}
          </Suspense>
        </AppFrame>
      </ToastProvider>
    </ThemeProvider>
  );
}
