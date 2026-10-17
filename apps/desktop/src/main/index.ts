import {
  app,
  BrowserWindow,
  dialog,
  globalShortcut,
  net,
  protocol,
  screen,
} from 'electron';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import {
  EventBus,
  Supervisor,
  encodeHostMessage,
  parseHelperLine,
  type SecretName,
} from '@qrtz/core';
import { createFileLogger } from './logging';
import { installCrashHandlers } from './crash';
import { ConfigStore } from './config-store';
import { SafeStorageSecretStore } from './secret-store';
import { WorkerClient } from './worker-client';
import { spawnHelper } from './helper-process';
import { hardenApp } from './security';
import { registerIpc } from './ipc';
import { MemoService } from './memo-service';

protocol.registerSchemesAsPrivileged([
  {
    scheme: 'qrtz-media',
    privileges: {
      standard: true,
      secure: true,
      stream: true,
      supportFetchAPI: true,
    },
  },
]);

export function bootstrap() {
  if (process.env.QRTZ_USER_DATA)
    app.setPath('userData', process.env.QRTZ_USER_DATA);
  const userData = app.getPath('userData');
  const config = new ConfigStore(path.join(userData, 'config.json'));
  const log = createFileLogger(
    path.join(userData, 'logs'),
    config.get().logLevel,
    !app.isPackaged,
  );
  installCrashHandlers(log);
  log.info('starting', {
    version: app.getVersion(),
    platform: process.platform,
  });

  const rendererDir = path.join(
    __dirname,
    `../renderer/${MAIN_WINDOW_VITE_NAME}`,
  );
  const isTrusted = (url: string) =>
    (MAIN_WINDOW_VITE_DEV_SERVER_URL
      ? url.startsWith(MAIN_WINDOW_VITE_DEV_SERVER_URL)
      : false) || url.startsWith(`file://${rendererDir}`);
  hardenApp(log, isTrusted);

  if (!app.requestSingleInstanceLock()) {
    app.quit();
    return;
  }

  const bus = new EventBus();
  const secrets = new SafeStorageSecretStore(
    path.join(userData, 'secrets.json'),
  );
  const worker = new WorkerClient(
    path.join(__dirname, 'worker.cjs'),
    [path.join(userData, 'qrtz.db')],
    log.child('worker'),
  );
  const helper = new Supervisor({
    name: 'helper',
    spawn: spawnHelper,
    logger: log.child('helper'),
  });
  helper.onStatus((s) => bus.emit('helper:status', s));
  const recordingsDir = path.join(userData, 'recordings');
  const memo = new MemoService({
    helper,
    worker,
    bus,
    log: log.child('memo'),
    secrets,
    config: () => config.get(),
    recordingsDir,
  });
  void memo.recover();
  helper.onLine((line) => {
    const r = parseHelperLine(line);
    if (!r.ok) return log.warn('bad helper line', { error: r.error });
    if (r.msg.type === 'log') log.child('helper')[r.msg.level](r.msg.msg);
    try {
      memo.handle(r.msg);
    } catch (e) {
      log.warn('memo handler failed', { err: String(e) });
    }
  });
  config.onChange((c) => bus.emit('config:changed', c));

  registerIpc({
    bus,
    log,
    isTrustedSender: isTrusted,
    handlers: {
      'app:get-info': () => ({
        name: app.getName(),
        version: app.getVersion(),
        platform: process.platform,
        arch: process.arch,
        electron: process.versions.electron ?? '',
      }),
      'config:get': () => config.get(),
      'config:set': ({ patch }) => config.set(patch),
      'secrets:set': async ({ name, value }) => {
        await secrets.set(name as SecretName, value);
        log.info('secret updated', { name, cleared: value === null });
        return { ok: true as const };
      },
      'secrets:list': () => secrets.list(),
      'db:health': () => worker.call('db.health', undefined),
      'permissions:get': () => memo.getPermissions(),
      'permissions:request': ({ permission }) =>
        memo.requestPermission(permission),
      'memo:session': () => memo.view(),
      'memo:start': (req) => memo.start(req?.title),
      'memo:stop': () => memo.stop(),
      'memo:dismiss': () => memo.dismiss(),
      'memo:list': (req) => worker.call('memo.list', req),
      'memo:get': ({ id }) => worker.call('memo.get', { id }),
      'memo:delete': async ({ id }) => {
        await worker.call('memo.delete', { id });
        bus.emit('memo:changed', { id });
        return { ok: true as const };
      },
      'memo:export': async ({ id, save }) => {
        const ex = await worker.call('memo.export', { id });
        if (!save) return ex;
        let target: string | undefined;
        if (process.env.QRTZ_EXPORT_DIR)
          target = path.join(process.env.QRTZ_EXPORT_DIR, ex.filename);
        else {
          const r = await dialog.showSaveDialog({
            defaultPath: path.join(app.getPath('documents'), ex.filename),
            filters: [{ name: 'Markdown', extensions: ['md'] }],
          });
          target = r.canceled ? undefined : r.filePath;
        }
        if (target) fs.writeFileSync(target, ex.markdown, 'utf8');
        return { ...ex, ...(target ? { savedTo: target } : {}) };
      },
      'helper:status': () => helper.status,
    },
  });

  const createWindow = () => {
    const win = new BrowserWindow({
      width: 1100,
      height: 720,
      minWidth: 720,
      minHeight: 480,
      title: 'qrtz',
      titleBarStyle: 'hiddenInset',
      backgroundColor: '#0a0a0a',
      show: false,
      webPreferences: {
        preload: path.join(__dirname, 'preload.cjs'),
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
        webSecurity: true,
        spellcheck: false,
      },
    });
    win.once('ready-to-show', () => win.show());
    if (MAIN_WINDOW_VITE_DEV_SERVER_URL)
      void win.loadURL(MAIN_WINDOW_VITE_DEV_SERVER_URL);
    else void win.loadFile(path.join(rendererDir, 'index.html'));
    if (!app.isPackaged && process.env.QRTZ_DEVTOOLS !== '0') {
      win.webContents.openDevTools({ mode: 'detach' });
    }
  };

  // M6.3: floating, always-on-top mini HUD while recording (index.html#hud, shell-scoped IPC).
  let hud: BrowserWindow | null = null;
  const showHud = () => {
    if (hud && !hud.isDestroyed()) return hud.showInactive();
    const area = screen.getPrimaryDisplay().workArea;
    hud = new BrowserWindow({
      width: 340,
      height: 56,
      x: area.x + area.width - 340 - 16,
      y: area.y + 16,
      frame: false,
      resizable: false,
      minimizable: false,
      maximizable: false,
      fullscreenable: false,
      skipTaskbar: true,
      alwaysOnTop: true,
      show: false,
      title: 'qrtz HUD',
      backgroundColor: '#111111',
      roundedCorners: true,
      webPreferences: {
        preload: path.join(__dirname, 'preload.cjs'),
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
        webSecurity: true,
        spellcheck: false,
      },
    });
    hud.setAlwaysOnTop(true, 'floating');
    hud.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
    hud.once('ready-to-show', () => hud?.showInactive());
    hud.on('closed', () => (hud = null));
    if (MAIN_WINDOW_VITE_DEV_SERVER_URL)
      void hud.loadURL(`${MAIN_WINDOW_VITE_DEV_SERVER_URL}#hud`);
    else
      void hud.loadFile(path.join(rendererDir, 'index.html'), { hash: 'hud' });
  };
  let hudHide: NodeJS.Timeout | null = null;
  bus.on('memo:session', (v) => {
    const want =
      config.get().memo.floatingHud &&
      (v.phase === 'recording' || v.phase === 'processing');
    if (want) {
      if (hudHide) clearTimeout(hudHide);
      hudHide = null;
      showHud();
    } else if (hud && !hudHide) {
      // Linger briefly so "Summarizing…" → done is visible, then close.
      hudHide = setTimeout(() => {
        hudHide = null;
        hud?.close();
      }, 1500);
    }
  });

  app.whenReady().then(() => {
    // Serve recordings to the sandboxed renderer: qrtz-media://rec/<uuid>.wav, nothing else.
    protocol.handle('qrtz-media', (req) => {
      const name = decodeURIComponent(new URL(req.url).pathname.slice(1));
      if (!/^[0-9a-f-]{36}\.wav$/.test(name))
        return new Response('forbidden', { status: 403 });
      return net.fetch(
        pathToFileURL(path.join(recordingsDir, name)).toString(),
        { headers: req.headers },
      );
    });
    worker.start();
    helper.start();
    createWindow();
    // M6.2: ⌘⇧R starts/stops Memo recording from anywhere (e.g. while in Zoom/Meet).
    const ok = globalShortcut.register('CommandOrControl+Shift+R', () => {
      memo
        .toggle()
        .catch((e) => log.warn('shortcut toggle failed', { err: String(e) }));
    });
    if (!ok) log.warn('global shortcut CommandOrControl+Shift+R unavailable');
    app.on('will-quit', () => globalShortcut.unregisterAll());
    app.on('activate', () => {
      if (BrowserWindow.getAllWindows().length === 0) createWindow();
    });
  });

  app.on('second-instance', () => {
    const w = BrowserWindow.getAllWindows()[0];
    if (w) {
      if (w.isMinimized()) w.restore();
      w.focus();
    }
  });
  app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') app.quit();
  });
  app.on('before-quit', () => {
    helper.send(encodeHostMessage({ type: 'shutdown' }));
    helper.stop();
    worker.stop();
    log.info('quit');
  });
}
