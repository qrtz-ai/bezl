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
import {
  AppFrame,
  SideNav,
  ThemeProvider,
  ToastProvider,
  type SideNavItem,
} from '@qrtz/ui';
import { isViewOn, onNavigateRequest } from '@qrtz/core';
import type { SubAppRoute } from '@qrtz/core/react';
import { registry } from './subapps';
import { SubAppHost } from './SubAppHost';
import { useConfigTheme } from './useConfigTheme';
import { useModuleSwitches } from './useModuleSwitches';
import { useAppUpdateDot } from './useAppUpdateDot';
import { useOnboarded } from './useOnboarded';
import {
  QRTZ,
  QRTZ_PARTS,
  hostOf,
  joinRoute,
  navTarget,
  pageOf,
  resolveRoute,
  splitRoute,
} from './routes';

// Not needed to paint the shell (ADR 0093): the palette opens on ⌘K (preloaded when idle); the permissions
// onboarding (ADR 0184) shows once, before the qrtz page, and again only when reopened from qrtz settings.
const loadPalette = () => import('./Palette');
const Palette = lazy(() => loadPalette().then((m) => ({ default: m.Palette })));
const Onboarding = lazy(() =>
  import('./Onboarding').then((m) => ({ default: m.Onboarding })),
);

/** Pinned to the bottom of the sidebar: only the dev-only gallery. There are no apps or settings tabs. */
export const SIDEBAR_FOOT = new Set(['gallery']);
/** The sub apps that make up the qrtz page (never a sidebar row of their own). */
export const QRTZ_PAGE_PARTS: ReadonlySet<string> = new Set(
  Object.values(QRTZ_PARTS),
);

/**
 * The sidebar: "qrtz" first (plain text, no icon: the app manager, with qrtz's settings behind its gear), then
 * the apps in registry order (echo, nano, ctrl, snap), then the dev-only gallery at the bottom.
 */
export function sidebarItems(
  visible: readonly { manifest: { id: string; name: string; icon?: string } }[],
  updateReady = false,
): SideNavItem[] {
  return [
    // ADR 0181: the dot says a newer qrtz is ready (settings → qrtz updates it).
    {
      id: QRTZ,
      label: 'qrtz',
      ...(updateReady ? { dot: true, dotLabel: 'qrtz update ready' } : {}),
    },
    ...visible
      .filter(({ manifest }) => !QRTZ_PAGE_PARTS.has(manifest.id))
      .map(({ manifest }) => ({
        id: manifest.id,
        label: manifest.name,
        ...(manifest.icon ? { icon: manifest.icon } : {}),
        group: SIDEBAR_FOOT.has(manifest.id)
          ? ('foot' as const)
          : ('main' as const),
      })),
  ];
}

/** One splat route: the shell owns the whole path and stays mounted across every navigation (ADR 0091). */
export const routes: RouteObject[] = [{ path: '*', element: <Shell /> }];

let router: ReturnType<typeof createHashRouter> | undefined;

export function App() {
  router ??= createHashRouter(routes);
  return <RouterProvider router={router} />;
}

export function Shell() {
  // ADR 0146: an app switched off on the qrtz page has no sidebar row and no view.
  const switches = useModuleSwitches();
  const visible = useMemo(
    () => registry.list().filter((a) => isViewOn(switches, a.manifest.id)),
    [switches],
  );
  // The pages below qrtz in the sidebar (the qrtz page's own parts are not pages).
  const apps = useMemo(
    () =>
      visible
        .map((a) => a.manifest.id)
        .filter((id) => !QRTZ_PAGE_PARTS.has(id)),
    [visible],
  );
  const updateDot = useAppUpdateDot();
  const location = useLocation();
  const navigate = useNavigate();
  const path = resolveRoute(location.pathname, apps);
  const { app: page, sub: pageSub } = splitRoute(path);
  // The sub app showing this page: on the qrtz page, the app manager or (at /qrtz/settings) the settings.
  const { host: active, sub } = hostOf(page, pageSub);
  // An unknown, malformed or old route (/marketplace…, /settings) is replaced by the canonical one.
  useEffect(() => {
    if (path !== location.pathname) void navigate(path, { replace: true });
  }, [path, location.pathname, navigate]);

  // Sub apps mount on first visit and then stay mounted (hidden) so switching back is instant; each keeps
  // the path it was last on, and each sidebar row returns to the page path it was last on.
  const [visited, setVisited] = useState<ReadonlySet<string>>(
    () => new Set([active]),
  );
  const [paths, setPaths] = useState<Readonly<Record<string, string>>>({});
  // Where the qrtz row returns to: its list or its settings (an app's modal isn't reopened).
  const [qrtzSub, setQrtzSub] = useState('');
  useEffect(() => {
    setVisited((v) => (v.has(active) ? v : new Set(v).add(active)));
    setPaths((p) => (p[active] === sub ? p : { ...p, [active]: sub }));
    if (page === QRTZ) setQrtzSub(pageSub.startsWith('item/') ? '' : pageSub);
  }, [active, sub, page, pageSub]);
  const pathsRef = useRef(paths);
  pathsRef.current = paths;
  const qrtzSubRef = useRef(qrtzSub);
  qrtzSubRef.current = qrtzSub;
  const activeRef = useRef(active);
  activeRef.current = active;

  const selectApp = useCallback(
    (id: string) =>
      void navigate(
        id === QRTZ
          ? joinRoute(QRTZ, qrtzSubRef.current)
          : joinRoute(id, pathsRef.current[id]),
      ),
    [navigate],
  );
  // Stable per app: sub apps put navigate in effect deps.
  const navFns = useRef(new Map<string, SubAppRoute['navigate']>());
  const navFor = (id: string) => {
    let f = navFns.current.get(id);
    if (!f) {
      f = (to, opts) => {
        if (activeRef.current === id) void navigate(pageOf(id, to), opts);
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
  // ADR 0184: qrtz settings → permissions reopens the onboarding over everything.
  const [reopened, setReopened] = useState(false);
  useEffect(
    () =>
      onNavigateRequest((r) =>
        r.onboarding ? setReopened(true) : void navigate(navTarget(r, apps)),
      ),
    [navigate, apps],
  );
  const theme = useConfigTheme();
  const [onboarded, setOnboarded] = useOnboarded();
  if (onboarded === false || reopened)
    return (
      <ThemeProvider mode={theme}>
        <ToastProvider>
          <Suspense fallback={null}>
            <Onboarding
              onDone={() => {
                setOnboarded(true);
                setReopened(false);
              }}
            />
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
          sidebar={
            <SideNav
              items={sidebarItems(visible, updateDot)}
              active={page}
              onSelect={selectApp}
            />
          }
        >
          {/* Only a first visit's module load reaches this boundary (it suspends inside the router's
              transition, so the previous app stays up); each sub app has its own boundary after that. */}
          <Suspense fallback={null}>
            {visible
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
