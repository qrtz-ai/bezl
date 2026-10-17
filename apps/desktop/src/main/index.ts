import { createTray } from './tray';
import { PendingDeletes } from './pending-deletes';
import {
  app,
  BrowserWindow,
  clipboard,
  dialog,
  globalShortcut,
  net,
  Notification,
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
import { helperAvailability, spawnHelper } from './helper-process';
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

/**
 * Test/dev hooks that must never steer a shipped build: fake data, fault injection, and redirecting the LLM
 * request (which carries the API key) to another host. Stripped before the worker/helper inherit the env.
 */
const DEV_ONLY_ENV = [
  'QRTZ_DEV_SEED',
  'QRTZ_LLM_BASE_URL',
  'QRTZ_SIMULATE_MISSING_HELPER',
  'QRTZ_MOCK_CRASH_AFTER_MS',
  'QRTZ_MOCK_SCENARIO',
  'QRTZ_BACKUP_PATH',
];

export function bootstrap() {
  if (app.isPackaged) for (const k of DEV_ONLY_ENV) delete process.env[k];
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
  let helperWasRunning = false;
  const availability = helperAvailability();
  const healthy = availability === 'native' ? 'ok' : availability;
  helper.onStatus((s) => {
    bus.emit('helper:status', s);
    if (availability !== 'missing') {
      if (s.state === 'running') memo.setHelperHealth(healthy);
      else if (s.state !== 'starting')
        memo.setHelperHealth('down', s.lastError ?? s.state);
    }
    if (s.state === 'running') helperWasRunning = true;
    else if (helperWasRunning && s.state !== 'starting') {
      helperWasRunning = false;
      memo.onHelperDown(s.lastError ?? s.state);
    }
    if (s.state === 'failed') memo.onHelperFailed(s.lastError ?? 'failed');
  });
  const recordingsDir = path.join(userData, 'recordings');
  const pendingDeletes = new PendingDeletes(
    userData,
    async (id) => {
      await worker.call('memo.delete', { id });
      log.info('deferred delete done', { id });
      bus.emit('memo:changed', { id });
    },
    (id, e) => log.warn('deferred delete failed', { id, err: String(e) }),
  );
  const memo = new MemoService({
    helper,
    worker,
    bus,
    log: log.child('memo'),
    secrets,
    config: () => config.get(),
    recordingsDir,
    ...(process.env.QRTZ_RETRY_SCALE && !app.isPackaged
      ? { retryScale: Number(process.env.QRTZ_RETRY_SCALE) }
      : {}),
    ...(process.env.QRTZ_FAULT_STEP && !app.isPackaged
      ? {
          faultStep: process.env.QRTZ_FAULT_STEP as
            | 'finalize'
            | 'summary'
            | 'index',
        }
      : {}),
    onDetectedPrompt: (title, platform) => {
      // The in-app prompt shows whenever detectPrompt is on; when qrtz isn't focused, also raise an OS notification.
      if (BrowserWindow.getFocusedWindow() || !Notification.isSupported())
        return;
      const n = new Notification({
        title: 'Meeting detected',
        body: `Record “${title || platform}”? Click to start recording.`,
        silent: true,
      });
      n.on('click', () => {
        void memo.start().catch(() => {});
        const w = BrowserWindow.getAllWindows()[0];
        if (w) {
          w.show();
          w.focus();
        }
      });
      n.show();
    },
  });
  if (availability === 'missing')
    memo.setHelperHealth(
      'missing',
      'The qrtz-helper binary is not in this build. Reinstall qrtz or build native/qrtz-helper.',
    );
  else if (availability === 'simulated') memo.setHelperHealth('simulated');
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
  /** Throttled (~10/s) backup progress events for the Settings progress bar. */
  const progress = (kind: 'export' | 'import') => {
    let last = 0;
    return (
      phase: 'checksum' | 'write' | 'verify',
      done: number,
      total: number,
    ) => {
      const now = Date.now();
      if (now - last < 100 && done < total) return;
      last = now;
      bus.emit('data:backup-progress', { kind, phase, done, total });
    };
  };

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
      'memo:pipeline': ({ id }) => memo.pipeline(id),
      'memo:retry-step': ({ id, step }) => memo.retryStep(id, step),
      'memo:live-transcript': (req) => memo.liveTranscript(req?.meetingId),
      'memo:list': async (req) => {
        const rows = await worker.call('memo.list', req);
        const gone = new Set(pendingDeletes.ids());
        return gone.size ? rows.filter((r) => !gone.has(r.meeting.id)) : rows;
      },
      'memo:get': ({ id }) => worker.call('memo.get', { id }),
      'memo:resummarize': ({ id }) => memo.resummarize(id),
      'memo:chat': (req) => memo.chat(req),
      'memo:set-auto-record': ({ enabled }) => ({
        autoRecord: config.set({ memo: { autoRecord: enabled } }).memo
          .autoRecord,
      }),
      'memo:rename-speaker': async (req) => {
        const r = await worker.call('memo.renameSpeaker', req);
        bus.emit('memo:changed', { id: req.meetingId });
        return r;
      },
      'memo:delete': async ({ id, deferMs }) => {
        if (deferMs) {
          pendingDeletes.schedule(id, deferMs);
          bus.emit('memo:changed', { id });
          return { ok: true as const };
        }
        await worker.call('memo.delete', { id });
        bus.emit('memo:changed', { id });
        return { ok: true as const };
      },
      'memo:undo-delete': ({ id }) => {
        const restored = pendingDeletes.undo(id);
        bus.emit('memo:changed', { id });
        return { restored };
      },
      'memo:export': async ({ id, save }) => {
        const ex = await worker.call('memo.export', { id });
        if (!save) {
          // Main-process clipboard: works even when the window isn't focused (navigator.clipboard doesn't).
          clipboard.writeText(ex.markdown);
          return ex;
        }
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
      'memo:add-note': async (req) => {
        const n = await memo.addNote(req);
        bus.emit('memo:changed', { id: n.meetingId });
        return n;
      },
      'memo:update-note': (req) => worker.call('memo.updateNote', req),
      'memo:delete-note': (req) => worker.call('memo.deleteNote', req),
      'memo:list-notes': async ({ meetingId }) => {
        const id = meetingId ?? memo.view().meetingId;
        return id ? worker.call('memo.listNotes', { meetingId: id }) : [];
      },
      'memo:set-template': async ({ id, template }) => {
        const r = await memo.setTemplate(id, template);
        bus.emit('memo:changed', { id });
        return r;
      },
      'memo:pause': ({ paused }) => memo.pause(paused),
      'hud:set-expanded': ({ expanded }) => {
        if (hud && !hud.isDestroyed()) {
          const [w] = hud.getSize();
          hud.setSize(w ?? 340, expanded ? HUD_EXPANDED_H : HUD_H, true);
          if (expanded) hud.focus();
        }
        return { expanded };
      },
      'data:export-backup': async () => {
        let target = process.env.QRTZ_BACKUP_PATH;
        if (!target) {
          const stamp = new Date()
            .toISOString()
            .slice(0, 16)
            .replace(/[:T]/g, '-');
          const r = await dialog.showSaveDialog({
            defaultPath: path.join(
              app.getPath('documents'),
              `qrtz-backup-${stamp}.zip`,
            ),
            filters: [{ name: 'qrtz backup', extensions: ['zip'] }],
          });
          if (r.canceled || !r.filePath)
            return { meetings: 0, audioFiles: 0, bytes: 0 };
          target = r.filePath;
        }
        const snap = path.join(
          app.getPath('temp'),
          `qrtz-snapshot-${process.pid}.db`,
        );
        try {
          const { meetings } = await worker.call('data.snapshot', {
            path: snap,
          });
          const health = await worker.call('db.health', undefined);
          const audioFiles = fs.existsSync(recordingsDir)
            ? fs
                .readdirSync(recordingsDir)
                .filter((f) => f.endsWith('.wav'))
                .map((f) => path.join(recordingsDir, f))
            : [];
          const r = await (
            await import('./backup')
          ).writeBackup({
            target,
            dbSnapshot: snap,
            audioFiles,
            appVersion: app.getVersion(),
            schemaVersion: health.schemaVersion,
            meetings,
            onProgress: progress('export'),
          });
          bus.emit('data:backup-progress', {
            kind: 'export',
            phase: 'done',
            done: 1,
            total: 1,
          });
          log.info('backup written', {
            meetings,
            audio: r.audioFiles,
            bytes: r.bytes,
          });
          return {
            savedTo: target,
            meetings,
            audioFiles: r.audioFiles,
            bytes: r.bytes,
          };
        } finally {
          fs.rmSync(snap, { force: true });
        }
      },
      'data:import-backup': async () => {
        let source = process.env.QRTZ_BACKUP_PATH;
        if (!source) {
          const r = await dialog.showOpenDialog({
            properties: ['openFile'],
            filters: [{ name: 'qrtz backup', extensions: ['zip'] }],
          });
          if (r.canceled || !r.filePaths[0])
            return { canceled: true, imported: 0, skipped: 0, audioFiles: 0 };
          source = r.filePaths[0];
        }
        const workDir = fs.mkdtempSync(
          path.join(app.getPath('temp'), 'qrtz-import-'),
        );
        try {
          const { dbPath, audioMap } = await (
            await import('./backup')
          ).readBackup({
            source,
            workDir,
            audioDir: recordingsDir,
            onProgress: progress('import'),
          });
          bus.emit('data:backup-progress', {
            kind: 'import',
            phase: 'import',
            done: 1,
            total: 1,
          });
          const r = await worker.call('data.importDb', {
            path: dbPath,
            audioMap,
          });
          log.info('backup imported', r);
          bus.emit('data:backup-progress', {
            kind: 'import',
            phase: 'done',
            done: 1,
            total: 1,
          });
          bus.emit('memo:changed', { id: '*' });
          return { ...r, audioFiles: Object.keys(audioMap).length };
        } finally {
          fs.rmSync(workDir, { recursive: true, force: true });
        }
      },
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
      void win.loadURL(`${MAIN_WINDOW_VITE_DEV_SERVER_URL}?dev=1`);
    else
      void win.loadFile(path.join(rendererDir, 'index.html'), {
        // Dev-only sub apps (the component gallery) are listed only in unpackaged builds.
        ...(app.isPackaged ? {} : { query: { dev: '1' } }),
      });
    if (!app.isPackaged && process.env.QRTZ_DEVTOOLS !== '0') {
      win.webContents.openDevTools({ mode: 'detach' });
    }
  };

  // M6.3: floating, always-on-top mini HUD while recording (index.html#hud, shell-scoped IPC).
  let hud: BrowserWindow | null = null;
  const HUD_H = 56;
  const HUD_EXPANDED_H = 220;
  const showHud = () => {
    if (hud && !hud.isDestroyed()) return hud.showInactive();
    const area = screen.getPrimaryDisplay().workArea;
    hud = new BrowserWindow({
      width: 340,
      height: HUD_H,
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
  if (!app.isPackaged) {
    // e2e: a trace of every lifecycle phase main broadcast (transient ones like `reconnecting` included).
    const g = globalThis as { __qrtzPhases?: string[] };
    g.__qrtzPhases = [];
    bus.on('memo:session', (v) => {
      if (g.__qrtzPhases!.at(-1) !== v.phase) g.__qrtzPhases!.push(v.phase);
    });
  }
  let hudHide: NodeJS.Timeout | null = null;
  bus.on('memo:session', (v) => {
    const want =
      config.get().memo.floatingHud &&
      [
        'starting',
        'recording',
        'reconnecting',
        'stopping',
        'processing',
      ].includes(v.phase);
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
    // After the worker is up: finish meetings a crash left behind (M5.1), then optional dev seed.
    const workerReady = worker.ready();
    void workerReady
      .then(() => memo.recover())
      .catch((e) => log.warn('startup recovery skipped', { err: String(e) }));
    if (process.env.QRTZ_DEV_SEED === 'many' && !app.isPackaged) {
      void workerReady
        .then(() =>
          worker.call('dev.seedMany', { count: 500, now: Date.now() }, 60_000),
        )
        .then(() => bus.emit('memo:changed', { id: '*' }))
        .catch((e) => log.warn('dev seed failed', { err: String(e) }));
    }
    if (process.env.QRTZ_DEV_SEED === 'large' && !app.isPackaged) {
      void workerReady
        .then(() =>
          worker.call(
            'dev.seedLarge',
            {
              id: '00000000-0000-4000-8000-000000000120',
              minutes: 120,
              startedAt: Date.now() - 3 * 3_600_000,
            },
            60_000,
          ),
        )
        .then(() =>
          bus.emit('memo:changed', {
            id: '00000000-0000-4000-8000-000000000120',
          }),
        )
        .catch((e) => log.warn('dev seed failed', { err: String(e) }));
    }
    void workerReady
      .then(() => pendingDeletes.recover())
      .then(() => bus.emit('memo:changed', { id: '*' }));
    helper.start();
    createWindow();
    // Menu bar item (macOS/Windows; Linux tray support varies by desktop, opt in with QRTZ_TRAY=1).
    // Settings → Menu bar icon toggles it live.
    let tray: ReturnType<typeof createTray> | null = null;
    const trayAllowed =
      process.platform !== 'linux' || process.env.QRTZ_TRAY === '1';
    const syncTray = (on: boolean) => {
      if (!trayAllowed) return;
      if (on && !tray)
        tray = createTray({
          getView: () => memo.view(),
          onAction: (a) => {
            const w =
              BrowserWindow.getAllWindows().find((x) => x !== hud) ?? null;
            if (a === 'start')
              void memo
                .start()
                .catch((e) =>
                  log.warn('tray start failed', { err: String(e) }),
                );
            else if (a === 'stop') void memo.stop().catch(() => {});
            else if (a === 'dismiss') memo.dismiss();
            else if (a === 'quit') app.quit();
            else if (w) {
              w.show();
              w.focus();
            } else createWindow();
          },
        });
      else if (!on && tray) {
        tray.destroy();
        tray = null;
      }
    };
    syncTray(config.get().app.menuBarIcon);
    config.onChange((c) => syncTray(c.app.menuBarIcon));
    bus.on('memo:session', () => tray?.update());
    app.on('before-quit', () => tray?.destroy());
    // Settings → Open at login (macOS/Windows login item; no-op elsewhere and in dev).
    const syncLogin = (on: boolean) => {
      if (!app.isPackaged || process.platform === 'linux') return;
      if (app.getLoginItemSettings().openAtLogin !== on)
        app.setLoginItemSettings({ openAtLogin: on });
    };
    syncLogin(config.get().app.launchAtLogin);
    config.onChange((c) => syncLogin(c.app.launchAtLogin));
    if (config.get().app.autoUpdateCheck)
      void import('./updates').then(({ initAutoUpdates }) =>
        initAutoUpdates(
          {
            isPackaged: app.isPackaged,
            platform: process.platform,
            arch: process.arch,
            env: process.env,
            build: {
              feedUrl: __QRTZ_UPDATE_FEED_URL__,
              autoUpdate: __QRTZ_AUTO_UPDATE__,
            },
          },
          log,
        ),
      );
    // M6.2: ⌘⇧R starts/stops Memo recording from anywhere (e.g. while in Zoom/Meet).
    const ok = globalShortcut.register('CommandOrControl+Shift+R', () => {
      memo
        .toggle()
        .catch((e) => log.warn('shortcut toggle failed', { err: String(e) }));
    });
    if (!ok) log.warn('global shortcut CommandOrControl+Shift+R unavailable');
    // Privacy: pause/resume capture from anywhere. Re-registered when the setting changes.
    let pauseKey = '';
    const bindPause = (key: string) => {
      if (key === pauseKey) return;
      if (pauseKey) globalShortcut.unregister(pauseKey);
      pauseKey = '';
      if (!key) return;
      try {
        const bound = globalShortcut.register(key, () => {
          try {
            memo.pause();
          } catch {
            /* not recording */
          }
        });
        if (bound) pauseKey = key;
        else log.warn('pause shortcut unavailable', { key });
      } catch (e) {
        log.warn('invalid pause shortcut', { key, err: String(e) });
      }
    };
    bindPause(config.get().privacy.pauseHotkey);
    config.onChange((c) => bindPause(c.privacy.pauseHotkey));
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
