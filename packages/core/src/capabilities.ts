import { z } from 'zod';

/** Fine-grained capabilities a sub app must declare to call IPC channels or receive events. */
export const Capability = z.enum([
  'app.info',
  'config.read',
  'config.write',
  'db.read',
  'helper.status',
  'os.permissions',
  'note.record',
  'note.read',
  'note.write',
  'llm.invoke',
  'data.backup',
  /** qrtz talk: read dictation state/history, start/stop dictation, edit history + dictionary. */
  'talk.read',
  'talk.dictate',
  'talk.write',
  /** qrtz speech (ADR 0046) and the on-device search model: read setup state / download, update, delete. */
  'speech.read',
  'speech.manage',
  /** ctrl (ADR 0103): read tasks and run history; create, pause, delete and run tasks. */
  'ctrl.read',
  'ctrl.write',
  /** qrtz snap (ADR 0159): what its helper reports (the shortcuts it holds, the Accessibility grant). */
  'snap.read',
  /**
   * ADR 0217 (H1): nano's Apple Calendar cache (status, upcoming events, refresh, calendar:changed). Its own
   * capability so a module holding note.read never sees the calendar.
   */
  'calendar.read',
  /**
   * ADR 0220 (M1): nano's notes vault (vault:get, doc:list, doc:get, doc:changed). Its own capability so the shell's
   * note.read (meetings, transcripts) never reaches the note files.
   */
  'doc.read',
  /** ADR 0228: Agent Dash's read-only view, and the user's controls (codes, approve, delete). dash's view only. */
  'dash.read',
  'dash.manage',
  /** ADR 0083: write a line to the local log (uncaught renderer errors, the audio player's errors). */
  'log.write',
  /** ADR 0083: save a copy of the local logs, or show their folder. */
  'logs.export',
  /** ADR 0146 M1.4: read and write the calling module's own settings (its settings page), nothing else. */
  'module.settings',
  /** ADR 0157: the Extensions screen: list installed packages; check, plan, apply and prune. */
  'packages.read',
  'packages.manage',
  /** "check for updates", "update" and "restart to update" for qrtz itself (ADR 0181), and the older index check. */
  'app.update',
  /** ADR 0181: read qrtz's own update state (settings → qrtz, the sidebar dot); checks and downloads nothing. */
  'app.update.read',
]);
export type Capability = z.infer<typeof Capability>;

/** macOS TCC permissions a sub app needs; the module manager shows them beside its switch (ADR 0163). */
export const OsPermission = z.enum([
  'accessibility',
  'screen-recording',
  'microphone',
]);
export type OsPermission = z.infer<typeof OsPermission>;

/**
 * The permissions the UI shows and asks for: the helper's three plus input monitoring, which qrtz-app reads and
 * asks for itself (snap's summon chord tap needs it). Off macOS it reads "unsupported".
 */
export const AppPermission = z.enum([
  ...OsPermission.options,
  'input-monitoring',
]);
export type AppPermission = z.infer<typeof AppPermission>;

/**
 * System Settings → Privacy & Security panes a permission prompt opens (`permissions:open-settings`): the ones qrtz
 * reads and asks for, plus those macOS only lets the user switch on there.
 */
export const SettingsPane = z.enum([
  'accessibility',
  'screen-recording',
  'microphone',
  'input-monitoring',
  'automation',
  'files-and-folders',
]);
export type SettingsPane = z.infer<typeof SettingsPane>;
