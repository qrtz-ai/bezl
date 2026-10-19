import { z } from 'zod';

/**
 * ADR 0221, TEMPORARY for tuning: the echo pill's (notch surface's) springs, fades and corners, as echo's "dev opts"
 * sliders set them (`talk.pillMotion`). The host forwards them in `dictation.configure{pillMotion}`; the helper's
 * `NotchMotion` (native/qrtz-helper/Sources/QrtzShared/Notch.swift) has the same names, defaults and ranges and
 * reads them on every animation. Defaults: the snappy spring. Bounce stops at 0.35 so the overshoot stays inside
 * the panel. Seconds, bounce 0..1, or pt.
 */
export const PILL_MOTION_PARAMS = [
  {
    key: 'expandDuration',
    label: 'expand duration',
    unit: 's',
    min: 0.05,
    max: 1.5,
    step: 0.01,
    def: 0.3,
  },
  {
    key: 'expandBounce',
    label: 'expand bounce',
    unit: '',
    min: 0,
    max: 0.35,
    step: 0.01,
    def: 0.15,
  },
  {
    key: 'collapseDuration',
    label: 'collapse duration',
    unit: 's',
    min: 0.05,
    max: 1.5,
    step: 0.01,
    def: 0.3,
  },
  {
    key: 'collapseBounce',
    label: 'collapse bounce',
    unit: '',
    min: 0,
    max: 0.35,
    step: 0.01,
    def: 0,
  },
  {
    key: 'fadeInDelay',
    label: 'fade in start',
    unit: 's',
    min: 0,
    max: 0.6,
    step: 0.01,
    def: 0.11,
  },
  {
    key: 'fadeInDuration',
    label: 'fade in duration',
    unit: 's',
    min: 0,
    max: 0.6,
    step: 0.01,
    def: 0.09,
  },
  {
    key: 'fadeOutDuration',
    label: 'fade out duration',
    unit: 's',
    min: 0,
    max: 0.1,
    step: 0.01,
    def: 0.06,
  },
  {
    key: 'widenDuration',
    label: 'widen duration',
    unit: 's',
    min: 0.05,
    max: 1.5,
    step: 0.01,
    def: 0.2,
  },
  {
    key: 'widenBounce',
    label: 'widen bounce',
    unit: '',
    min: 0,
    max: 0.35,
    step: 0.01,
    def: 0.04,
  },
  {
    key: 'radiusCollapsed',
    label: 'radius collapsed',
    unit: 'pt',
    min: 0,
    max: 16,
    step: 0.5,
    def: 8,
  },
  {
    key: 'radiusExpanded',
    label: 'radius expanded',
    unit: 'pt',
    min: 0,
    max: 32,
    step: 0.5,
    def: 14,
  },
  {
    key: 'shoulder',
    label: 'shoulder radius',
    unit: 'pt',
    min: 0,
    max: 16,
    step: 0.5,
    def: 8,
  },
] as const;

export type PillMotionKey = (typeof PILL_MOTION_PARAMS)[number]['key'];
export type PillMotion = Record<PillMotionKey, number>;

export const PILL_MOTION_DEFAULTS = Object.fromEntries(
  PILL_MOTION_PARAMS.map((p) => [p.key, p.def]),
) as PillMotion;

export const PillMotionSchema = z
  .object(
    Object.fromEntries(
      PILL_MOTION_PARAMS.map((p) => [
        p.key,
        z.number().min(p.min).max(p.max).default(p.def),
      ]),
    ) as Record<PillMotionKey, z.ZodDefault<z.ZodNumber>>,
  )
  .prefault({});

/** A full, clamped motion object from whatever is stored (missing or bad values take the default). */
export function pillMotion(v: unknown): PillMotion {
  const o = (typeof v === 'object' && v !== null ? v : {}) as Record<
    string,
    unknown
  >;
  return Object.fromEntries(
    PILL_MOTION_PARAMS.map((p) => {
      const n = o[p.key];
      return [
        p.key,
        typeof n === 'number' && Number.isFinite(n)
          ? Math.min(p.max, Math.max(p.min, n))
          : p.def,
      ];
    }),
  ) as PillMotion;
}
