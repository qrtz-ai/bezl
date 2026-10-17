import {
  createContext,
  useContext,
  useInsertionEffect,
  type ReactNode,
} from 'react';
import { globalCss, tokens, type Tokens } from './tokens';

const ThemeContext = createContext<Tokens>(tokens);

/** Injects global CSS (vars, reset, keyframes) once and provides tokens to components. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  useInsertionEffect(() => {
    if (document.getElementById('q-global')) return;
    const style = document.createElement('style');
    style.id = 'q-global';
    style.textContent = globalCss(tokens);
    document.head.appendChild(style);
  }, []);
  return (
    <ThemeContext.Provider value={tokens}>{children}</ThemeContext.Provider>
  );
}

export const useTheme = (): Tokens => useContext(ThemeContext);
