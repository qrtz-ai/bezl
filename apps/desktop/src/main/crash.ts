import { app, crashReporter } from 'electron';
import type { Logger } from '@qrtz/core';

/** Hook point for a remote crash/error service (e.g. Sentry). Local-only by default. */
export interface CrashSink {
  capture(kind: string, details: Record<string, unknown>): void;
}

export function installCrashHandlers(log: Logger, sink?: CrashSink) {
  // Native minidumps stay local (userData/Crashpad) unless uploads are configured later.
  crashReporter.start({ uploadToServer: false, compress: true });
  const report = (kind: string, details: Record<string, unknown>) => {
    log.error(kind, details);
    sink?.capture(kind, details);
  };
  process.on('uncaughtException', (err) =>
    report('uncaughtException', { err }),
  );
  process.on('unhandledRejection', (reason) =>
    report('unhandledRejection', { err: reason }),
  );
  app.on('render-process-gone', (_e, wc, d) =>
    report('render-process-gone', { url: wc.getURL(), ...d }),
  );
  app.on('child-process-gone', (_e, d) =>
    report('child-process-gone', { ...d }),
  );
}
