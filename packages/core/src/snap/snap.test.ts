import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  SNAP_ACTIONS,
  SNAP_DEFAULT_SHORTCUTS,
  bindingFromKey,
  shortcutSymbols,
} from './contract';

const swift = readFileSync(
  join(
    import.meta.dirname,
    '../../../../native/qrtz-helper/Sources/QrtzSnap/SnapShortcuts.swift',
  ),
  'utf8',
);

describe('qrtz snap (ADR 0159)', () => {
  it("has spectacle's defaults, the same table as qrtz-snap", () => {
    for (const a of SNAP_ACTIONS)
      expect(swift).toContain(`.${a}: "${SNAP_DEFAULT_SHORTCUTS[a]}"`);
    expect(new Set(Object.values(SNAP_DEFAULT_SHORTCUTS)).size).toBe(18);
  });
  it('draws shortcuts with mac modifier symbols', () => {
    expect(shortcutSymbols('alt+cmd+left')).toBe('⌥⌘ left');
    expect(shortcutSymbols('ctrl+alt+shift+right')).toBe('⌃⌥⇧ right');
    expect(shortcutSymbols('alt+cmd+c')).toBe('⌥⌘c');
    expect(shortcutSymbols('')).toBe('');
  });
  it('records a key press as a binding', () => {
    const k = { ctrlKey: false, altKey: true, shiftKey: false, metaKey: true };
    expect(bindingFromKey({ ...k, key: 'ArrowLeft', code: 'ArrowLeft' })).toBe(
      'alt+cmd+left',
    );
    expect(bindingFromKey({ ...k, key: 'ç', code: 'KeyC' })).toBe('alt+cmd+c');
    expect(bindingFromKey({ ...k, key: 'Meta', code: 'MetaLeft' })).toBeNull();
  });
});
