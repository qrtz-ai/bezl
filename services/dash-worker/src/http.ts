/** JSON responses: `readme_version` in every body, HSTS and no caching on every response. */
import { README_VERSION } from './constants';

export type Scope = 'widget' | 'agent' | 'dashboard';
export type Json =
  | string
  | number
  | boolean
  | null
  | Json[]
  | { [k: string]: Json };
export type JsonObj = { [k: string]: Json };
export interface Out {
  status: number;
  body: JsonObj;
}

export const SECURITY_HEADERS: Record<string, string> = {
  'strict-transport-security': 'max-age=63072000; includeSubDomains; preload',
  'x-content-type-options': 'nosniff',
  'cache-control': 'no-store',
  'referrer-policy': 'no-referrer',
  'content-security-policy': "default-src 'none'; frame-ancestors 'none'",
};

export const out = (
  status: number,
  body: Record<string, Json | undefined>,
): Out => ({
  status,
  body: {
    ...(JSON.parse(JSON.stringify(body)) as JsonObj),
    readme_version: README_VERSION,
  },
});
export const fail = (status: number, error: string) => out(status, { error });
export const stop = (status: number, error: string) =>
  out(status, { error, action: 'stop' });
export const gone = (scope: Scope) =>
  out(410, { error: 'deleted', action: 'stop', scope });

export const toResponse = (o: Out) => {
  const headers: Record<string, string> = {
    ...SECURITY_HEADERS,
    'content-type': 'application/json; charset=utf-8',
  };
  const retry = o.body.retry_after_ms;
  if (o.status === 429 && typeof retry === 'number')
    headers['retry-after'] = String(Math.ceil(retry / 1000));
  return new Response(JSON.stringify(o.body), { status: o.status, headers });
};
