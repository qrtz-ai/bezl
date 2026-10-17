import { Children, type ReactNode } from 'react';

/**
 * Branding rule: every title in qrtz is fully lowercase (headings, section/page/dialog titles, tabs, HUD/pill
 * labels). Title-bearing components run their title through this. Only literal strings are changed; elements and
 * user content passed with `preserveCase` stay as they are. scripts/check-titles.mjs keeps the source strings
 * lowercase too, so screen readers and window titles match.
 */
export const lowerTitle = (node: ReactNode): ReactNode =>
  typeof node === 'string'
    ? node.toLowerCase()
    : Array.isArray(node)
      ? Children.map(node, (c) => (typeof c === 'string' ? c.toLowerCase() : c))
      : node;
