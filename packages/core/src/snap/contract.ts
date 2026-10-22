import { z } from 'zod';

/**
 * qrtz snap (ADR 0159): Spectacle's window snapping as a qrtz module. The actions, their labels and Spectacle's default
 * shortcuts; native/qrtz-helper/Sources/QrtzSnap/SnapShortcuts.swift has the same table (snap.test.ts reads it).
 */
export const SNAP_ACTIONS = [
  'center',
  'fullscreen',
  'leftHalf',
  'rightHalf',
  'topHalf',
  'bottomHalf',
  'upperLeft',
  'lowerLeft',
  'upperRight',
  'lowerRight',
  'nextDisplay',
  'previousDisplay',
  'nextThird',
  'previousThird',
  'makeLarger',
  'makeSmaller',
  'undo',
  'redo',
] as const;
export const SnapAction = z.enum(SNAP_ACTIONS);
export type SnapAction = z.infer<typeof SnapAction>;

export const SNAP_DEFAULT_SHORTCUTS: Readonly<Record<SnapAction, string>> = {
  center: 'alt+cmd+c',
  fullscreen: 'alt+cmd+f',
  leftHalf: 'alt+cmd+left',
  rightHalf: 'alt+cmd+right',
  topHalf: 'alt+cmd+up',
  bottomHalf: 'alt+cmd+down',
  upperLeft: 'ctrl+cmd+left',
  lowerLeft: 'ctrl+shift+cmd+left',
  upperRight: 'ctrl+cmd+right',
  lowerRight: 'ctrl+shift+cmd+right',
  nextDisplay: 'ctrl+alt+cmd+right',
  previousDisplay: 'ctrl+alt+cmd+left',
  nextThird: 'ctrl+alt+right',
  previousThird: 'ctrl+alt+left',
  makeLarger: 'ctrl+alt+shift+right',
  makeSmaller: 'ctrl+alt+shift+left',
  undo: 'alt+cmd+z',
  redo: 'alt+shift+cmd+z',
};

export const SNAP_LABELS: Readonly<Record<SnapAction, string>> = {
  center: 'center',
  fullscreen: 'fullscreen',
  leftHalf: 'left half',
  rightHalf: 'right half',
  topHalf: 'top half',
  bottomHalf: 'bottom half',
  upperLeft: 'upper left',
  lowerLeft: 'lower left',
  upperRight: 'upper right',
  lowerRight: 'lower right',
  nextDisplay: 'next display',
  previousDisplay: 'previous display',
  nextThird: 'next third',
  previousThird: 'previous third',
  makeLarger: 'make larger',
  makeSmaller: 'make smaller',
  undo: 'undo last move',
  redo: 'redo last move',
};

/** The groups Spectacle's preferences window shows. */
export const SNAP_GROUPS: readonly (readonly SnapAction[])[] = [
  ['center', 'fullscreen'],
  ['leftHalf', 'rightHalf', 'topHalf', 'bottomHalf'],
  ['upperLeft', 'lowerLeft', 'upperRight', 'lowerRight'],
  ['nextDisplay', 'previousDisplay'],
  ['nextThird', 'previousThird'],
  ['makeLarger', 'makeSmaller'],
  ['undo', 'redo'],
];

const MODIFIERS = ['ctrl', 'alt', 'shift', 'cmd'] as const;
const SYMBOLS: Record<string, string> = {
  ctrl: '⌃',
  alt: '⌥',
  shift: '⇧',
  cmd: '⌘',
  escape: 'esc',
  forwarddelete: 'del',
};

/** "alt+cmd+left" to "⌥⌘ left" (Spectacle's modifier order: ⌃⌥⇧⌘; keys stay words). "" stays "". */
export function shortcutSymbols(binding: string): string {
  if (!binding) return '';
  const parts = binding.split('+');
  const key = parts[parts.length - 1] ?? '';
  const mods = MODIFIERS.filter((m) => parts.slice(0, -1).includes(m));
  const k = SYMBOLS[key] ?? key;
  return mods.map((m) => SYMBOLS[m]).join('') + (k.length > 1 ? ` ${k}` : k);
}

const KEY_NAMES: Record<string, string> = {
  ArrowLeft: 'left',
  ArrowRight: 'right',
  ArrowUp: 'up',
  ArrowDown: 'down',
  Enter: 'return',
  ' ': 'space',
  Tab: 'tab',
  Escape: 'escape',
  Backspace: 'delete',
  Delete: 'forwarddelete',
  Home: 'home',
  End: 'end',
  PageUp: 'pageup',
  PageDown: 'pagedown',
};

/**
 * A recorded key press as a binding ("ctrl+alt+right"), or null for a bare modifier or a key snap can't bind.
 * Uses `code` for letters and digits so ⌥ (which changes `key` on a Mac) still records the key itself.
 */
export function bindingFromKey(e: {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}): string | null {
  let key: string | undefined = KEY_NAMES[e.key];
  if (!key && /^Key[A-Z]$/.test(e.code)) key = e.code.slice(3).toLowerCase();
  if (!key && /^Digit[0-9]$/.test(e.code)) key = e.code.slice(5);
  if (!key && /^F([1-9]|1[0-9]|20)$/.test(e.key)) key = e.key.toLowerCase();
  if (!key) return null;
  const mods = [
    e.ctrlKey && 'ctrl',
    e.altKey && 'alt',
    e.shiftKey && 'shift',
    e.metaKey && 'cmd',
  ].filter(Boolean);
  return [...mods, key].join('+');
}

/** What snap's helper last reported (`snap:status`): the shortcuts it holds and the ones it couldn't. */
export const SnapStatus = z.object({
  /** qrtz-snap is running (it only runs while snap is on). */
  running: z.boolean(),
  /** qrtz has the Accessibility grant snap moves windows with (ADR 0154: one grant for every extension). */
  axTrusted: z.boolean(),
  registered: z.record(z.string(), z.string()),
  /** Why an action has no shortcut: another app holds the keys, two actions share them, or it needs a modifier. */
  refused: z.record(z.string(), z.string()),
  /** Summon agent: qrtz has Input Monitoring (the ⌘+⌘ chord's listen-only tap needs it). */
  inputMonitoring: z.boolean().optional(),
  /** Summon agent: the chord tap is listening. */
  summonActive: z.boolean().optional(),
  /** Summon agent's picker: installed apps (only while summon is on). */
  apps: z
    .array(z.object({ bundleId: z.string(), name: z.string() }))
    .optional(),
  /** Echo's push-to-talk key is right ⌘, half of summon's chord. */
  summonClash: z.boolean().optional(),
});
export type SnapStatus = z.infer<typeof SnapStatus>;
