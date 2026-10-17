import {
  HELPER_PROTOCOL_VERSION,
  HostToHelper,
  type HelperToHost,
} from './protocol';
import type { ChildHandle, Clock } from './supervisor';
import { systemClock } from './supervisor';

/**
 * In-process TypeScript stand-in for the Swift helper. Speaks the same JSON-lines protocol.
 * Used in tests and on non-macOS hosts (dev on Linux).
 */
export function createMockHelper(
  opts: { clock?: Clock; heartbeatMs?: number; os?: string } = {},
): ChildHandle & {
  crash(code?: number): void;
} {
  const clock = opts.clock ?? systemClock;
  const lineCbs: ((l: string) => void)[] = [];
  const exitCbs: ((c: number | null, s: string | null) => void)[] = [];
  let alive = true;
  let hb: unknown = null;
  const emit = (m: HelperToHost) => {
    if (alive) for (const cb of lineCbs) cb(JSON.stringify(m));
  };
  const beat = () => {
    emit({ type: 'heartbeat', ts: clock.now() });
    hb = clock.setTimeout(beat, opts.heartbeatMs ?? 5000);
  };
  const die = (code: number | null, signal: string | null) => {
    if (!alive) return;
    alive = false;
    if (hb) clock.clearTimeout(hb);
    for (const cb of exitCbs) cb(code, signal);
  };
  clock.setTimeout(() => {
    emit({
      type: 'hello',
      protocol: HELPER_PROTOCOL_VERSION,
      helperVersion: 'mock',
      os: opts.os ?? 'mock',
      features: ['mock'],
    });
    beat();
  }, 0);
  return {
    onLine: (cb) => void lineCbs.push(cb),
    onExit: (cb) => void exitCbs.push(cb),
    write(line) {
      const parsed = HostToHelper.safeParse(JSON.parse(line));
      if (!parsed.success)
        return emit({
          type: 'error',
          code: 'E_PROTOCOL',
          message: 'bad message',
          fatal: false,
        });
      const m = parsed.data;
      if (m.type === 'ping') emit({ type: 'pong', id: m.id });
      else if (
        m.type === 'permissions.check' ||
        m.type === 'permissions.request'
      )
        emit({
          type: 'permissions.status',
          status: {
            accessibility: 'unsupported',
            'screen-recording': 'unsupported',
            microphone: 'unsupported',
          },
        });
      else if (m.type === 'shutdown') die(0, null);
    },
    kill: () => die(null, 'SIGTERM'),
    crash: (code = 1) => die(code, null),
  };
}
