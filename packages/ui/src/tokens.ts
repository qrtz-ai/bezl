export const tokens = {
  color: {
    bg: '#0b0b0f',
    surface: '#15151c',
    border: '#26262f',
    text: '#f5f5f7',
    textMuted: '#9a9aa5',
    accent: '#7c6cff',
    accentText: '#ffffff',
  },
  space: { 1: '4px', 2: '8px', 3: '12px', 4: '16px', 6: '24px', 8: '32px' },
  radius: { sm: '6px', md: '10px' },
  font: {
    family:
      "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif",
    size: { sm: '12px', md: '14px', lg: '18px', xl: '28px' },
  },
} as const;

export type Tokens = typeof tokens;
