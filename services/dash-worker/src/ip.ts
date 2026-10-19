/**
 * Client address → limiter buckets (Bozniak M2 on 60416b7). IPv4 is keyed on the full /32. IPv6 is keyed on its
 * /64 (one customer's LAN; an attacker gets 2^64 addresses there for free), with a looser count on the /48 (one
 * site's allocation). IPv4-mapped IPv6 (::ffff:a.b.c.d) counts as the IPv4 address. Anything that doesn't parse
 * as an address is refused by the caller, never pooled into a shared bucket.
 */
export type Source = {
  /** Strict bucket: an IPv4 /32 or an IPv6 /64. */
  narrow: string;
  /** Looser bucket: the IPv6 /48; null for IPv4. */
  wide: string | null;
};

const V4 =
  /^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/;

const v4 = (s: string): string | null => (V4.test(s) ? s : null);

/** The 8 hextets of an IPv6 address, or null. Accepts '::' and a trailing dotted IPv4. No zone ids. */
const v6 = (s: string): number[] | null => {
  if (s.length > 45 || !/^[0-9a-fA-F:.]+$/.test(s)) return null;
  let tail: number[] | null = null;
  if (s.includes('.')) {
    const lastColon = s.lastIndexOf(':');
    const quad = v4(s.slice(lastColon + 1));
    if (!quad) return null;
    const b = quad.split('.').map(Number);
    tail = [(b[0]! << 8) | b[1]!, (b[2]! << 8) | b[3]!];
    s = `${s.slice(0, lastColon + 1)}0:0`;
  }
  const halves = s.split('::');
  if (halves.length > 2) return null;
  const part = (h: string) =>
    h === ''
      ? []
      : h
          .split(':')
          .map((x) => (/^[0-9a-fA-F]{1,4}$/.test(x) ? parseInt(x, 16) : NaN));
  const head = part(halves[0]!);
  const rest = halves.length === 2 ? part(halves[1]!) : [];
  if ([...head, ...rest].some(Number.isNaN)) return null;
  const want = 8;
  if (halves.length === 1) {
    if (head.length !== want) return null;
    return tail ? [...head.slice(0, 6), ...tail] : head;
  }
  const gap = want - head.length - rest.length;
  if (gap < 1) return null;
  const all = [...head, ...Array<number>(gap).fill(0), ...rest];
  return tail ? [...all.slice(0, 6), ...tail] : all;
};

const hex = (h: number[]) => h.map((x) => x.toString(16)).join(':');

/** The limiter buckets for a CF-Connecting-IP value, or null if it is missing or not an address. */
export const sourceOf = (raw: string | null): Source | null => {
  if (raw === null) return null;
  const s = raw.trim();
  const a = v4(s);
  if (a) return { narrow: `v4:${a}`, wide: null };
  const h = v6(s);
  if (!h) return null;
  if (h.slice(0, 5).every((x) => x === 0) && h[5] === 0xffff)
    return {
      narrow: `v4:${h[6]! >> 8}.${h[6]! & 255}.${h[7]! >> 8}.${h[7]! & 255}`,
      wide: null,
    };
  return {
    narrow: `v6/64:${hex(h.slice(0, 4))}`,
    wide: `v6/48:${hex(h.slice(0, 3))}`,
  };
};
