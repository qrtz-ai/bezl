import fs from 'node:fs';
import path from 'node:path';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import {
  NOTCH_GEOMETRY,
  NOTCH_ICONS,
  NotchSurface,
  notchPath,
} from './NotchSurface';

const swift = (rel: string) =>
  fs.readFileSync(
    path.resolve(__dirname, '../../../native/qrtz-helper/Sources', rel),
    'utf8',
  );

describe('NotchSurface (ADR 0218)', () => {
  it('draws the same ten-step notch outline as Swift NotchShape.path', () => {
    // NotchTests.testShapeHasAFlatTopRoundedBottomAndConcaveEars, as an SVG path.
    expect(notchPath(200, 40, 12, 8)).toBe(
      'M0 0 Q8 0 8 8 L8 28 Q8 40 20 40 L180 40 Q192 40 192 28 L192 8 Q192 0 200 0 L0 0 Z',
    );
    // The fake tab's rest: zero height, everything on the top edge.
    expect(notchPath(180, 0, 8, 8)).toBe(
      'M0 0 Q0 0 0 0 L0 0 Q0 0 0 0 L180 0 Q180 0 180 0 L180 0 Q180 0 180 0 L0 0 Z',
    );
  });

  it('keeps its geometry and icon set in step with the Swift side', () => {
    const src = swift('QrtzShared/Notch.swift');
    for (const [k, v] of Object.entries(NOTCH_GEOMETRY)) {
      expect(src, k).toMatch(new RegExp(`\\b${k} = ${v}(\\.0)?\\b`));
    }
    const icons = /enum NotchIcon[^{]*\{\s*case ([^\n]+)/.exec(src)?.[1];
    expect(icons?.split(',').map((s) => s.trim())).toEqual([...NOTCH_ICONS]);
  });

  it('renders every variant without clickable parts, attributed module content first', () => {
    const html = renderToStaticMarkup(
      <NotchSurface
        source="nano"
        content={{ kind: 'progress', label: 'downloading', value: 0.5 }}
      />,
    );
    expect(html).toContain('data-kind="progress"');
    expect(html).toContain('pointer-events:none');
    expect(html.indexOf('nano')).toBeLessThan(html.indexOf('downloading'));
    expect(html).not.toMatch(/<(a|button)\b/);
    for (const c of [
      { kind: 'level', busy: false },
      { kind: 'status', text: 'x', tone: 'error' },
      { kind: 'icon', icon: 'bolt', label: 'x' },
    ] as const) {
      expect(renderToStaticMarkup(<NotchSurface content={c} />)).toContain(
        `data-kind="${c.kind}"`,
      );
    }
  });
});
