// Tailwind + shadcn variables (scoped: tw: prefix, no preflight), loaded with ThemeProvider so every window that
// renders @qrtz/ui gets it (a barrel import would be tree-shaken: sideEffects). Consumers wire @tailwindcss/vite.
import './styles.css';
import {
  createContext,
  useContext,
  useEffect,
  useInsertionEffect,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';
import {
  globalCss,
  tokensToCssVars,
  themeTokens,
  tokens,
  type ThemeMode,
  type Tokens,
} from './tokens';
import fontUrl from './fonts/GeistPixel-Square.woff2?url';

const ThemeContext = createContext<Tokens>(tokens);

export type ThemePreference = ThemeMode | 'system';

function useSystemMode(): ThemeMode {
  const query = () =>
    typeof matchMedia === 'function' &&
    matchMedia('(prefers-color-scheme: light)').matches
      ? 'light'
      : 'dark';
  const [mode, setMode] = useState<ThemeMode>(query);
  useEffect(() => {
    if (typeof matchMedia !== 'function') return;
    const mq = matchMedia('(prefers-color-scheme: light)');
    const on = () => setMode(mq.matches ? 'light' : 'dark');
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, []);
  return mode;
}

/** Resolves a preference ('system' follows the OS) to a palette. */
export function useResolvedMode(pref: ThemePreference = 'dark'): ThemeMode {
  const system = useSystemMode();
  return pref === 'system' ? system : pref;
}

/** Injects global CSS (vars, reset, focus ring, keyframes) and provides tokens. Dark is the brand default. */
export function ThemeProvider({
  children,
  mode = 'dark',
}: {
  children: ReactNode;
  mode?: ThemePreference;
}) {
  const resolved = useResolvedMode(mode);
  const t = themeTokens(resolved);
  useInsertionEffect(() => {
    let style = document.getElementById('q-global');
    if (!style) {
      style = document.createElement('style');
      style.id = 'q-global';
      document.head.appendChild(style);
    }
    style.textContent = globalCss(t, fontUrl);
    // shadcn's dark: variant follows the qrtz theme (styles.css @custom-variant), not the OS setting.
    document.documentElement.dataset.qMode = t.mode;
    document.documentElement.dataset.theme = resolved;
  }, [resolved]);
  return <ThemeContext.Provider value={t}>{children}</ThemeContext.Provider>;
}

/** Renders children in another palette without touching the page (gallery previews). */
export function ThemeScope({
  mode,
  children,
  style,
}: {
  mode: ThemeMode;
  children: ReactNode;
  style?: CSSProperties;
}) {
  const t = themeTokens(mode);
  return (
    <ThemeContext.Provider value={t}>
      <div
        data-theme={mode}
        // Scope the CSS variables too, so shadcn's var-driven classes and its dark: variant follow this palette.
        data-q-mode={mode}
        style={{
          ...(tokensToCssVars(t) as CSSProperties),
          background: t.color.bg,
          color: t.color.text,
          colorScheme: mode,
          ...style,
        }}
      >
        {children}
      </div>
    </ThemeContext.Provider>
  );
}

export const useTheme = (): Tokens => useContext(ThemeContext);
