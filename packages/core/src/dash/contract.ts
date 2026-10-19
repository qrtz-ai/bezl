import { z } from 'zod';

/**
 * Agent Dash (ADR 0228): a read-only dashboard the user's agents write to. Rust (crates/qrtz-dash) is the server;
 * these are its block schemas (the same rules as its serde types) and the read-only view the `dash:*` channels return.
 */
/** The one origin agents talk to. Placeholder until the server ships (no DNS or deploy yet). */
export const DASH_ORIGIN = 'https://dash.qrtz.ai';
export const DASH_README_VERSION = '2.1';
export const DASH_RETENTION_DAYS = 30;
export const DASH_BATCH_MAX_OPS = 20;
export const DASH_STALE_AFTER_MS = 10 * 60 * 1000;

const text = (max = 500) => z.string().max(max);
const finite = z.number().refine(Number.isFinite);

/** https only, a real host, ':' only before a numeric port, no userinfo (the Rust `is_https_url`). */
export const isHttpsUrl = (u: string) => {
  const m = /^https:\/\/([^/?#]*)/.exec(u);
  if (!m || /\s/.test(u) || [...u].some((c) => c.charCodeAt(0) < 0x20))
    return false;
  const [host = '', port, ...extra] = (m[1] ?? '').split(':');
  if (extra.length) return false;
  if (port !== undefined) {
    const n = Number(port);
    if (!/^\d{1,5}$/.test(port) || n < 1 || n > 65535) return false;
  }
  return (
    host.includes('.') &&
    host.split('.').every((l) => /^[A-Za-z0-9-]+$/.test(l) && !/^-|-$/.test(l))
  );
};
/** A punycode (`xn--`) label: the link may imitate another name (the Rust `is_punycode_host`). */
export const isPunycodeHost = (u: string) => {
  const host = (/^https:\/\/([^/?#:]*)/.exec(u)?.[1] ?? '').toLowerCase();
  return host.split('.').some((l) => l.startsWith('xn--'));
};
/** Safe markdown subset: no html, autolinks or images. */
export const isSafeMarkdown = (s: string) => !/<[A-Za-z/!]|!\[/.test(s);

export const DashBlockType = z.enum([
  'status',
  'stat',
  'progress',
  'list',
  'table',
  'log',
  'chart',
  'markdown',
]);
export type DashBlockType = z.infer<typeof DashBlockType>;

export const DashBlocks = {
  status: z
    .object({
      status: z.enum([
        'running',
        'blocked',
        'waiting_on_user',
        'done',
        'error',
      ]),
      text: text().optional(),
    })
    .strict(),
  stat: z
    .object({
      value: finite,
      label: text().optional(),
      unit: text().optional(),
      delta: finite.optional(),
    })
    .strict(),
  progress: z
    .object({
      done: z.number().int().nonnegative(),
      total: z.number().int().nonnegative(),
      step: text().optional(),
    })
    .strict()
    .refine((p) => p.done <= p.total),
  list: z
    .object({
      items: z
        .array(
          z
            .object({
              text: text(),
              url: z.string().refine(isHttpsUrl).optional(),
              state: text().optional(),
            })
            .strict(),
        )
        .max(50),
    })
    .strict(),
  table: z
    .object({
      columns: z.array(text()).max(20),
      rows: z.array(z.array(z.union([text(), finite]))).max(100),
    })
    .strict()
    .refine((t) => t.rows.every((r) => r.length === t.columns.length)),
  log: z
    .object({
      line: text(),
      level: z.enum(['info', 'warn', 'error']).optional(),
    })
    .strict(),
  chart: z.object({ value: finite }).strict(),
  markdown: z
    .object({ text: z.string().max(2000).refine(isSafeMarkdown) })
    .strict(),
} as const satisfies Record<DashBlockType, z.ZodType>;

/** Parse a stored block for display; anything that doesn't fit its type is not shown. */
export const parseDashBlock = (type: DashBlockType, data: unknown) => {
  const r = DashBlocks[type].safeParse(data);
  return r.success ? r.data : null;
};

export const DashWidget = z.object({
  key: z.string(),
  type: DashBlockType,
  title: z.string(),
  /** Untrusted agent data: re-parsed with `parseDashBlock` before it is shown. */
  state: z.unknown().nullable(),
  events: z.array(z.object({ at: z.number(), data: z.unknown() })).max(50),
  updatedAt: z.number(),
});
export type DashWidget = z.infer<typeof DashWidget>;

export const DashAgent = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string(),
  fingerprint: z.string(),
  status: z.enum(['pending', 'approved']),
  heartbeat: z.enum(['running', 'idle']).nullable(),
  lastSeen: z.number(),
  stale: z.boolean(),
  /** The secret scanner redacted something this agent sent. */
  flagged: z.boolean(),
  createdAt: z.number(),
  widgets: z.array(DashWidget).max(20),
});
export type DashAgent = z.infer<typeof DashAgent>;

export const DashView = z.object({
  origin: z.string(),
  readmeVersion: z.string(),
  deleted: z.boolean(),
  retentionDays: z.number(),
  agents: z.array(DashAgent).max(50),
  audit: z
    .array(
      z.object({
        event: z.string(),
        agentId: z.string().nullable(),
        at: z.number(),
      }),
    )
    .max(50),
});
export type DashView = z.infer<typeof DashView>;

/** The user's only controls that reach an agent (as a 410). */
export const DashDelete = z.discriminatedUnion('scope', [
  z.object({
    scope: z.literal('widget'),
    agentId: z.string(),
    key: z.string(),
  }),
  z.object({ scope: z.literal('agent'), agentId: z.string() }),
  z.object({ scope: z.literal('dashboard') }),
]);
export type DashDelete = z.infer<typeof DashDelete>;
