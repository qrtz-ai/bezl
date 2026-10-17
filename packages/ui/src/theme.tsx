import { createContext, useContext, type ReactNode } from 'react';
import { tokens, type Tokens } from './tokens';

const ThemeContext = createContext<Tokens>(tokens);

export function ThemeProvider({ children }: { children: ReactNode }) {
  return (
    <ThemeContext.Provider value={tokens}>
      <div
        style={{
          height: '100%',
          background: tokens.color.bg,
          color: tokens.color.text,
          fontFamily: tokens.font.family,
          fontSize: tokens.font.size.md,
        }}
      >
        {children}
      </div>
    </ThemeContext.Provider>
  );
}

export const useTheme = (): Tokens => useContext(ThemeContext);
