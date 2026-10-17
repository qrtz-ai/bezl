import { app, session, shell } from 'electron';
import type { Logger } from '@qrtz/core';

/** App-wide Electron hardening (see docs/security.md). */
export function hardenApp(log: Logger, isTrustedUrl: (url: string) => boolean) {
  app.on('web-contents-created', (_e, contents) => {
    contents.setWindowOpenHandler(({ url }) => {
      if (/^https:\/\//.test(url)) void shell.openExternal(url);
      return { action: 'deny' };
    });
    contents.on('will-navigate', (e, url) => {
      if (!isTrustedUrl(url)) {
        log.warn('blocked navigation', { url });
        e.preventDefault();
      }
    });
    contents.on('will-attach-webview', (e) => e.preventDefault());
  });
  app.whenReady().then(() => {
    // The renderer never needs camera/mic/geo/etc: capture happens in the native helper.
    session.defaultSession.setPermissionRequestHandler(
      (_wc, permission, cb) => {
        log.warn('denied permission request', { permission });
        cb(false);
      },
    );
    session.defaultSession.setPermissionCheckHandler(() => false);
  });
}
