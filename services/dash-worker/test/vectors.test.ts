import { env } from 'cloudflare:workers';
import { describe, expect, it } from 'vitest';
import { isHttpsUrl, validateAppend, validatePut } from '../src/blocks';
import * as C from '../src/constants';
import { CODE_ALPHABET, codeChars, hmacRaw } from '../src/crypto';
import { cleanName, skeleton, validKey } from '../src/names';
import { scrub, scrubValue } from '../src/scrub';

const V = JSON.parse(env.TEST_VECTORS) as {
  scrub: { in: string; out: string; hit: boolean }[];
  https_url: { url: string; ok: boolean }[];
  hmac: { key_hex: string; msg: string; mac_hex: string }[];
};
const hex = (h: string) =>
  new Uint8Array(h.match(/../g)!.map((x) => parseInt(x, 16)));

describe('vectors shared with crates/qrtz-dash (contracts/dash/vectors.json)', () => {
  it('scrub', () => {
    for (const v of V.scrub)
      expect(scrub(v.in), JSON.stringify(v.in)).toEqual({
        text: v.out,
        hit: v.hit,
      });
  });
  it('is_https_url', () => {
    for (const v of V.https_url) expect(isHttpsUrl(v.url), v.url).toBe(v.ok);
  });
  it('hmac', async () => {
    for (const v of V.hmac)
      expect(
        await hmacRaw(hex(v.key_hex), new TextEncoder().encode(v.msg)),
      ).toBe(v.mac_hex);
  });
});

describe('the scanner (Bozniak M1)', () => {
  it('scans object keys', () => {
    const r = scrubValue({
      ghp_abcdefghijklmnopqrstuvwx: 1,
      ok: { 'sk-abcdefghijklmnop1234': 'x' },
    });
    expect(r).toEqual({
      hit: true,
      value: { '[redacted]': 1, ok: { '[redacted]': 'x' } },
    });
  });
});

describe('codes', () => {
  it('are uniform over the alphabet (rejection sampling)', () => {
    const counts = Array.from({ length: 30 }, () => 0);
    for (let i = 0; i < 3000; i++)
      for (const c of codeChars(8)) {
        const i = CODE_ALPHABET.indexOf(c);
        counts[i] = (counts[i] ?? 0) + 1;
      }
    const lo = counts.slice(0, 16).reduce((a, b) => a + b) / 16;
    const hi = counts.slice(16).reduce((a, b) => a + b) / 14;
    expect(Math.abs(lo / hi - 1)).toBeLessThan(0.06);
  });
});

describe('blocks and names match the Rust tests', () => {
  it('every block accepts its spec example and refuses extras', () => {
    const ok: [Parameters<typeof validatePut>[0], Record<string, unknown>][] = [
      ['status', { status: 'blocked', text: 'Waiting on User ID' }],
      ['stat', { value: 3, label: 'open', unit: 'pr', delta: -1 }],
      ['progress', { done: 2, total: 5, step: 'upload' }],
      [
        'list',
        { items: [{ text: 'a', url: 'https://example.com/x', state: 'done' }] },
      ],
      ['table', { columns: ['a', 'b'], rows: [['x', 1]] }],
      ['markdown', { text: '**hi** [docs](https://qrtz.ai)' }],
    ];
    for (const [t, v] of ok) {
      expect(validatePut(t, v), t).toBeNull();
      expect(validatePut(t, { ...v, action: 'click me' }), t).not.toBeNull();
    }
    expect(validatePut('status', { status: 'done', text: null })).toBeNull();
    expect(validateAppend('log', { line: 'x', level: 'warn' })).toBeNull();
    expect(validateAppend('chart', { value: 1.5 })).toBeNull();
    expect(validatePut('log', { line: 'x' })).not.toBeNull();
    expect(validateAppend('status', { status: 'done' })).not.toBeNull();
  });
  it('refuses limits and unsafe content', () => {
    expect(validatePut('status', { status: 'needs_you' })).not.toBeNull();
    expect(validatePut('progress', { done: 6, total: 5 })).not.toBeNull();
    expect(
      validatePut('list', {
        items: Array.from({ length: 51 }, () => ({ text: 'x' })),
      }),
    ).not.toBeNull();
    for (const m of [
      '<b>x</b>',
      '![i](https://a.com/i.png)',
      '<https://a.com>',
      '<!-- x -->',
    ])
      expect(validatePut('markdown', { text: m }), m).not.toBeNull();
    expect(validatePut('markdown', { text: 'a < b' })).toBeNull();
    expect(validatePut('markdown', { text: 'x'.repeat(2001) })).not.toBeNull();
    expect(
      validatePut('table', { columns: ['a'], rows: [['x', 'y']] }),
    ).not.toBeNull();
    expect(
      validatePut('status', { status: 'done', text: '😀'.repeat(500) }),
    ).toBeNull();
  });
  it('names fold confusables and strip bidi', () => {
    expect(cleanName('Grok\u202E Bot\u200B')).toBe('Grok Bot');
    expect(skeleton('Adm1n')).toBe(skeleton('admin'));
    expect(skeleton('Sуstеm')).toBe(skeleton('system'));
    expect(
      validKey('state-farm') &&
        !validKey('State') &&
        !validKey('a--b') &&
        !validKey(''),
    ).toBe(true);
  });
});

describe('constants are the Rust twins (crates/qrtz-dash/src/lib.rs)', () => {
  it('match', () => {
    const lib = env.TEST_RUST_LIB;
    const rust = (name: string) => {
      const m = new RegExp(`pub const ${name}: [^=]+= ([^;]+);`).exec(lib);
      expect(m, name).not.toBeNull();
      const e = m![1]!.trim();
      // Only string literals and products of integers appear there (e.g. `15 * 60 * 1000`).
      return e.startsWith('"')
        ? JSON.parse(e)
        : e
            .split('*')
            .reduce((a, x) => a * Number(x.trim().replace(/_/g, '')), 1);
    };
    for (const k of [
      'DASH_ORIGIN',
      'README_VERSION',
      'RETENTION_DAYS',
      'BATCH_MAX_OPS',
      'ENROLL_TTL_MS',
      'STALE_AFTER_MS',
      'MAX_BODY_BYTES',
      'MAX_BATCH_BODY_BYTES',
      'MAX_CONTENT_BYTES',
      'WRITES_PER_MIN_AGENT',
      'WRITES_PER_MIN_DASHBOARD',
      'MAX_AGENTS',
      'MAX_WIDGETS_PER_AGENT',
      'LOG_RING',
      'CHART_RING',
      'IDEMPOTENCY_TTL_MS',
      'DAY_MS',
    ] as const)
      expect(C[k], k).toBe(rust(k));
    // The enroll-failure limits are in the crate once Bozniak's M2 fix lands there.
    for (const k of [
      'ENROLL_FAILS_PER_MIN',
      'ENROLL_FAILS_BEFORE_ROTATION',
    ] as const)
      if (lib.includes(`pub const ${k}`)) expect(C[k], k).toBe(rust(k));
  });
});
