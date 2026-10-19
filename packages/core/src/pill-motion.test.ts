import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { ConfigSchema } from './config';
import {
  PILL_MOTION_DEFAULTS,
  PILL_MOTION_PARAMS,
  pillMotion,
} from './pill-motion';

describe('talk.pillMotion (ADR 0221, temporary pill tuning)', () => {
  it('defaults to the snappy spring in a fresh config', () => {
    const c = ConfigSchema.parse({});
    expect(c.talk.pillMotion).toEqual(PILL_MOTION_DEFAULTS);
  });
  it('rejects out-of-range values on write; pillMotion() clamps whatever is stored', () => {
    expect(() =>
      ConfigSchema.parse({ talk: { pillMotion: { expandBounce: 2 } } }),
    ).toThrow();
    expect(pillMotion({ expandBounce: 2, shoulder: -1, x: 1 })).toEqual({
      ...PILL_MOTION_DEFAULTS,
      expandBounce: 0.35,
      shoulder: 0,
    });
  });
  it('matches the helper NotchMotion names, defaults and ranges', () => {
    const swift = fs.readFileSync(
      path.resolve(
        __dirname,
        '../../../native/qrtz-helper/Sources/QrtzShared/Notch.swift',
      ),
      'utf8',
    );
    for (const p of PILL_MOTION_PARAMS) {
      expect(swift).toContain(`("${p.key}", ${p.min}, ${p.max})`);
    }
  });
});
