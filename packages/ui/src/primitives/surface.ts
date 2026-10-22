import type { CSSProperties } from 'react';
import type { Tokens } from '../tokens';
import { tokens } from '../tokens';

/** shadcn `popover` surface (menu, select content): one border, the overlay shadow, md radius. */
export const popoverSurface = (t: Tokens): CSSProperties => ({
  zIndex: t.z.overlay,
  minWidth: 180,
  display: 'grid',
  gap: tokens.space[0.5],
  padding: t.space[1],
  background: t.color.surface,
  border: `1px solid ${t.color.borderStrong}`,
  borderRadius: t.radius.md,
  boxShadow: tokens.shadow.overlay,
  outline: 'none',
});

/** shadcn menu/select item row; highlight comes from `[data-highlighted]` in globalCss. */
export const itemRow = (t: Tokens, disabled?: boolean): CSSProperties => ({
  boxSizing: 'border-box',
  cursor: disabled ? 'not-allowed' : 'pointer',
  opacity: disabled ? 0.5 : 1,
  display: 'flex',
  alignItems: 'center',
  gap: t.space[2],
  height: 28,
  padding: `0 ${t.space[2]}`,
  borderRadius: t.radius.sm,
  color: t.color.text,
  outline: 'none',
  userSelect: 'none',
});
