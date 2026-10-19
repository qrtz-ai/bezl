/**
 * The eight typed blocks, rule for rule crates/qrtz-dash/src/blocks.rs (serde `deny_unknown_fields` + limits).
 * Written by hand rather than with zod so the edge cases match serde: an optional field may be null, lengths count
 * Unicode scalar values (Rust `chars().count()`), integers must be integers.
 */
import { isRustWhitespace } from './scrub';

export const BLOCK_TYPES = [
  'status',
  'stat',
  'progress',
  'list',
  'table',
  'log',
  'chart',
  'markdown',
] as const;
export type BlockType = (typeof BLOCK_TYPES)[number];
export const parseBlockType = (s: unknown): BlockType | null =>
  typeof s === 'string' && (BLOCK_TYPES as readonly string[]).includes(s)
    ? (s as BlockType)
    : null;
export const isAppend = (t: BlockType) => t === 'log' || t === 'chart';

export const TEXT_MAX = 500;
export const LIST_MAX = 50;
export const TABLE_ROWS_MAX = 100;
export const TABLE_COLS_MAX = 20;
export const MARKDOWN_MAX = 2000;

class BlockError extends Error {}
const fail = (m: string): never => {
  throw new BlockError(m);
};

type Obj = Record<string, unknown>;
const obj = (v: unknown, fields: Record<string, boolean>): Obj => {
  if (!v || typeof v !== 'object' || Array.isArray(v))
    fail('invalid payload: expected an object');
  const o = v as Obj;
  for (const k of Object.keys(o))
    if (!(k in fields)) fail(`invalid payload: unknown field \`${k}\``);
  for (const [k, required] of Object.entries(fields))
    if (required && !(k in o)) fail(`invalid payload: missing field \`${k}\``);
  return o;
};
const present = (v: unknown) => v !== undefined && v !== null;
const len = (s: string) => [...s].length;
const str = (v: unknown, what: string, max = TEXT_MAX): string => {
  if (typeof v !== 'string') fail(`invalid payload: ${what} is a string`);
  if (len(v as string) > max) fail(`${what} is longer than ${max} characters`);
  return v as string;
};
const optStr = (v: unknown, what: string) =>
  present(v) ? str(v, what) : undefined;
const num = (v: unknown, what: string) => {
  if (typeof v !== 'number' || !Number.isFinite(v))
    fail(`${what} must be a finite number`);
  return v as number;
};
const u64 = (v: unknown, what: string) => {
  if (typeof v !== 'number' || !Number.isInteger(v) || v < 0)
    fail(`invalid payload: ${what} is a whole number`);
  return v as number;
};
const oneOf = (v: unknown, allowed: readonly string[], what: string) => {
  if (typeof v !== 'string' || !allowed.includes(v))
    fail(`invalid payload: unknown ${what}`);
};
const isControl = (c: string) => {
  const n = c.codePointAt(0) ?? 0;
  return n <= 0x1f || (n >= 0x7f && n <= 0x9f);
};

/** `https://<host>[:port][/...]`, no userinfo, ':' only before a numeric port 1-65535 (Bozniak L5). */
export const isHttpsUrl = (u: string) => {
  if (!u.startsWith('https://')) return false;
  if ([...u].some((c) => isRustWhitespace(c) || isControl(c))) return false;
  const rest = u.slice(8);
  const authority = rest.split(/[/?#]/)[0] ?? '';
  const i = authority.indexOf(':');
  const host = i < 0 ? authority : authority.slice(0, i);
  const port = i < 0 ? null : authority.slice(i + 1);
  const portOk =
    port === null ||
    (/^\d{1,5}$/.test(port) && Number(port) >= 1 && Number(port) <= 65535);
  return (
    portOk &&
    host.includes('.') &&
    host
      .split('.')
      .every(
        (l) =>
          l.length > 0 &&
          !l.startsWith('-') &&
          !l.endsWith('-') &&
          /^[A-Za-z0-9-]+$/.test(l),
      )
  );
};

const markdownOk = (s: string) => {
  str(s, 'text', MARKDOWN_MAX);
  for (let i = 0; i < s.length; i++) {
    const n = s[i + 1];
    if (
      s[i] === '<' &&
      n !== undefined &&
      (/[A-Za-z]/.test(n) || n === '/' || n === '!')
    )
      fail("markdown can't contain html or autolinks");
    if (s[i] === '!' && n === '[') fail("markdown can't contain images");
  }
};

const check = (f: () => void): string | null => {
  try {
    f();
    return null;
  } catch (e) {
    if (e instanceof BlockError) return e.message;
    throw e;
  }
};

/** null when `v` is a valid PUT payload for `ty`, else the reason. */
export const validatePut = (ty: BlockType, v: unknown): string | null =>
  check(() => {
    switch (ty) {
      case 'status': {
        const o = obj(v, { status: true, text: false });
        oneOf(
          o.status,
          ['running', 'blocked', 'waiting_on_user', 'done', 'error'],
          'status',
        );
        optStr(o.text, 'text');
        return;
      }
      case 'stat': {
        const o = obj(v, {
          value: true,
          label: false,
          unit: false,
          delta: false,
        });
        num(o.value, 'value');
        if (present(o.delta)) num(o.delta, 'delta');
        optStr(o.label, 'label');
        optStr(o.unit, 'unit');
        return;
      }
      case 'progress': {
        const o = obj(v, { done: true, total: true, step: false });
        const done = u64(o.done, 'done');
        const total = u64(o.total, 'total');
        optStr(o.step, 'step');
        if (done > total) fail('done is more than total');
        return;
      }
      case 'list': {
        const o = obj(v, { items: true });
        if (!Array.isArray(o.items)) fail('invalid payload: items is a list');
        const items = o.items as unknown[];
        const parsed = items.map((it) =>
          obj(it, { text: true, url: false, state: false }),
        );
        for (const it of parsed) {
          str(it.text, 'text');
          if (present(it.url) && typeof it.url !== 'string')
            fail('invalid payload: url is a string');
        }
        if (items.length > LIST_MAX)
          fail(`a list has at most ${LIST_MAX} items`);
        for (const it of parsed) {
          str(it.text, 'text');
          optStr(it.state, 'state');
          if (present(it.url) && !isHttpsUrl(it.url as string))
            fail('list urls must be https://');
        }
        return;
      }
      case 'table': {
        const o = obj(v, { columns: true, rows: true });
        if (!Array.isArray(o.columns) || !Array.isArray(o.rows))
          fail('invalid payload: columns and rows are lists');
        const cols = o.columns as unknown[];
        const rows = o.rows as unknown[];
        for (const c of cols)
          if (typeof c !== 'string')
            fail('invalid payload: a column is a string');
        for (const r of rows) {
          if (!Array.isArray(r)) fail('invalid payload: a row is a list');
          for (const c of r as unknown[])
            if (typeof c !== 'string' && typeof c !== 'number')
              fail('invalid payload: a cell is a string or number');
        }
        if (rows.length > TABLE_ROWS_MAX || cols.length > TABLE_COLS_MAX)
          fail(
            `a table has at most ${TABLE_ROWS_MAX} rows and ${TABLE_COLS_MAX} columns`,
          );
        for (const c of cols) str(c, 'column');
        for (const r of rows as unknown[][]) {
          if (r.length !== cols.length)
            fail('every row has one cell per column');
          for (const c of r) {
            if (typeof c === 'string') str(c, 'cell');
            else num(c, 'cell');
          }
        }
        return;
      }
      case 'markdown': {
        const o = obj(v, { text: true });
        if (typeof o.text !== 'string')
          fail('invalid payload: text is a string');
        markdownOk(o.text as string);
        return;
      }
      case 'log':
      case 'chart':
        fail('this widget takes events, not put');
    }
  });

/** null when `v` is a valid appended event for `ty` (log or chart), else the reason. */
export const validateAppend = (ty: BlockType, v: unknown): string | null =>
  check(() => {
    if (ty === 'log') {
      const o = obj(v, { line: true, level: false });
      if (present(o.level)) oneOf(o.level, ['info', 'warn', 'error'], 'level');
      str(o.line, 'line');
    } else if (ty === 'chart') {
      num(obj(v, { value: true }).value, 'value');
    } else fail('this widget takes put, not events');
  });
