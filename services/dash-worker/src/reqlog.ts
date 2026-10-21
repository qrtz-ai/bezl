/**
 * QRT-307 (Bozniak ruling 6): the only request logging. One JSON line per request with the method, the route
 * template, the status and the time taken. Never the real path, query string, headers or bodies: path segments
 * that carry ids, keys or anything agent-written are replaced by `:name`, and anything that is not a known route
 * is logged as `unmatched`.
 */
const STATIC = new Set([
  '/v1/readme',
  '/v1/enroll',
  '/v1/heartbeat',
  '/v1/widgets',
  '/v1/batch',
  '/v1/owner/setup',
  '/v1/owner/test-codes',
  '/v1/owner/live',
  '/v1/owner/view',
  '/v1/owner/codes',
  '/v1/owner/codes/rotate',
  '/v1/owner/dashboard',
]);
const METHODS = new Set([
  'GET',
  'POST',
  'PUT',
  'DELETE',
  'PATCH',
  'HEAD',
  'OPTIONS',
]);

/** The route template for a request path, e.g. `/v1/widgets/:key/events`. Never returns path text it was given. */
export function routeTemplate(path: string): string {
  if (STATIC.has(path)) return path;
  const p = path.split('/');
  // p[0] is '' for a leading slash.
  if (p[0] !== '' || p[1] !== 'v1' || p.slice(2).some((s) => s === ''))
    return 'unmatched';
  if (p[2] === 'widgets' && p.length === 4) return '/v1/widgets/:key';
  if (p[2] === 'widgets' && p.length === 5 && p[4] === 'events')
    return '/v1/widgets/:key/events';
  if (p[2] === 'owner' && p[3] === 'agents' && p.length === 5)
    return '/v1/owner/agents/:id';
  if (
    p[2] === 'owner' &&
    p[3] === 'agents' &&
    p.length === 6 &&
    p[5] === 'review'
  )
    return '/v1/owner/agents/:id/review';
  if (p[2] === 'owner' && p[3] === 'widgets' && p.length === 6)
    return '/v1/owner/widgets/:agent/:key';
  return 'unmatched';
}

export type RequestLine = {
  method: string;
  route: string;
  status: number;
  ms: number;
};

export function requestLine(
  method: string,
  path: string,
  status: number,
  ms: number,
): RequestLine {
  return {
    method: METHODS.has(method) ? method : 'OTHER',
    route: routeTemplate(path),
    status,
    ms: Math.max(0, Math.round(ms)),
  };
}
