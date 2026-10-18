import { AppError, LOG_MESSAGE_MAX, type IpcRequest } from '@qrtz/core';

/**
 * ADR 0083: the renderer's errors go to the local log (`log:write`, target `renderer` in qrtz.log): uncaught
 * errors, unhandled rejections and the audio player's media errors, from every window. Only error text and code
 * locations are sent: never a transcript, dictated text, note, summary or title. Quoted strings longer than a
 * short identifier are replaced by their length (an engine's error can quote the value it choked on), the media
 * error never names the file, and the backend caps and rate-limits again.
 */
type LogWrite = IpcRequest<'log:write'>;
export interface LogSink {
  invoke(channel: 'log:write', req: LogWrite): Promise<unknown>;
}

/** Client-side budget per window (the backend's limit is shared by all windows). */
export const RENDERER_LOG_PER_MINUTE = 30;
const QUOTE_KEEP = 24;
const MESSAGE_MAX = 300;

/** Long quoted spans → `"[n chars]"`; whitespace runs collapsed; cut to `max`. */
export function scrub(s: string, max = MESSAGE_MAX): string {
  const q = s.replace(/"[^"]*"|'[^']*'|“[^”]*”|`[^`]*`/g, (m) =>
    m.length - 2 > QUOTE_KEEP ? `"[${m.length - 2} chars]"` : m,
  );
  const flat = q.replace(/\s+/g, ' ').trim();
  return flat.length > max ? `${flat.slice(0, max)}…` : flat;
}

/** `https://host/assets/x.js:1:2` → `assets/x.js:1:2`: the bundle path and position only. */
const frame = (l: string) =>
  l
    .trim()
    .replace(/^at\s+/, '')
    .replace(/^[a-z][a-z0-9+.-]*:\/\/[^/]*\//i, '')
    .replace(/\((?:[a-z][a-z0-9+.-]*:\/\/[^/]*\/)?/i, '(');

export function describeError(e: unknown): string {
  if (e instanceof AppError) return `${e.code}: ${scrub(e.message)}`;
  if (e instanceof Error) {
    const frames = (e.stack ?? '')
      .split('\n')
      .filter((l) => /:\d+:\d+\)?\s*$/.test(l))
      .slice(0, 3)
      .map(frame);
    const head = `${e.name}: ${scrub(e.message)}`;
    return frames.length ? `${head} @ ${frames.join(' < ')}` : head;
  }
  if (typeof e === 'string') return scrub(e);
  return `non-error rejection (${e === null ? 'null' : typeof e})`;
}

const MEDIA_ERR = ['', 'aborted', 'network', 'decode', 'src-not-supported'];

/** The player's error without the file it was loading (the scheme only). */
export function describeMediaError(el: HTMLMediaElement): string {
  const err = el.error;
  const code = err?.code ?? 0;
  const src = el.currentSrc || el.src || '';
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(src)?.[1] ?? 'none';
  const msg = err?.message ? ` ${scrub(err.message, 160)}` : '';
  return `${el.tagName.toLowerCase()} error ${code} (${MEDIA_ERR[code] ?? 'unknown'})${msg}; src ${scheme}:; network ${el.networkState}, ready ${el.readyState}`;
}

/** Install the listeners on `win`; returns an uninstall function. `scope` names the window (main, hud). */
export function installRendererLogging(
  sink: LogSink,
  scope: string,
  win: Window = window,
  now: () => number = Date.now,
): () => void {
  let windowStart = 0;
  let sent = 0;
  let last = '';
  let lastAt = 0;
  const send = (level: LogWrite['level'], s: string, message: string) => {
    const t = now();
    if (message === last && t - lastAt < 5_000) return;
    if (t - windowStart >= 60_000) {
      windowStart = t;
      sent = 0;
    }
    if (sent >= RENDERER_LOG_PER_MINUTE) return;
    sent += 1;
    last = message;
    lastAt = t;
    // Logging must never surface an error of its own.
    void sink
      .invoke('log:write', {
        level,
        scope: s,
        message: message.slice(0, LOG_MESSAGE_MAX),
      })
      .catch(() => {});
  };
  const onError = (ev: Event) => {
    const target = ev.target;
    if (
      typeof HTMLMediaElement !== 'undefined' &&
      target instanceof HTMLMediaElement
    ) {
      send('error', 'player', describeMediaError(target));
    } else if (ev instanceof ErrorEvent) {
      const at = ev.filename
        ? ` @ ${frame(ev.filename)}:${ev.lineno}:${ev.colno}`
        : '';
      send(
        'error',
        `${scope}.uncaught`,
        ev.error !== undefined && ev.error !== null
          ? describeError(ev.error)
          : `${scrub(ev.message)}${at}`,
      );
    }
  };
  const onRejection = (ev: PromiseRejectionEvent) =>
    send('error', `${scope}.rejection`, describeError(ev.reason));
  // Capture phase: media `error` events don't bubble, but they do pass the window on the way down.
  win.addEventListener('error', onError, true);
  win.addEventListener('unhandledrejection', onRejection);
  return () => {
    win.removeEventListener('error', onError, true);
    win.removeEventListener('unhandledrejection', onRejection);
  };
}
