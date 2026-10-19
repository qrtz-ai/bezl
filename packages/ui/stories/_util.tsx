import { useState, type ReactNode } from 'react';
import { useTheme } from '../src';

export const noop = () => {};

/** A row of examples with a lowercase caption. */
export function Row({
  label,
  children,
}: {
  label?: string;
  children: ReactNode;
}) {
  const t = useTheme();
  return (
    <div style={{ display: 'grid', gap: t.space[2], marginBottom: t.space[5] }}>
      {label ? (
        <span style={{ color: t.color.textMuted, fontSize: 12 }}>{label}</span>
      ) : null}
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: t.space[3],
          alignItems: 'center',
          minWidth: 0,
        }}
      >
        {children}
      </div>
    </div>
  );
}

/** A fixed-size frame for whole-window components. */
export function Frame({
  children,
  height = 320,
}: {
  children: ReactNode;
  height?: number;
}) {
  const t = useTheme();
  return (
    <div
      style={{
        width: '100%',
        height,
        position: 'relative',
        overflow: 'hidden',
        border: `1px solid ${t.color.border}`,
        borderRadius: t.radius.md,
        transform: 'translateZ(0)',
      }}
    >
      {children}
    </div>
  );
}

export function Width({
  w = 360,
  children,
}: {
  w?: number;
  children: ReactNode;
}) {
  return <div style={{ width: w, maxWidth: '100%' }}>{children}</div>;
}

export function useToggle(init = false) {
  return useState(init);
}
