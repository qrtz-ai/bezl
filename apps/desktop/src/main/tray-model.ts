import type { SessionView } from '@qrtz/core';

export type TrayAction = 'open' | 'start' | 'stop' | 'dismiss' | 'quit';
export interface TrayItem {
  label: string;
  action?: TrayAction;
  enabled?: boolean;
  separator?: boolean;
}

const clock = (ms: number) => {
  const t = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const s = String(t % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
};

/** Pure tray state: icon, menu-bar title, tooltip and menu for a session (unit-tested; Electron wiring in tray.ts). */
export function trayState(v: SessionView, now = Date.now()) {
  const recording = v.phase === 'recording';
  const elapsed = recording && v.startedAt ? now - v.startedAt : v.elapsedMs;
  const status =
    v.helper === 'missing'
      ? 'Recording unavailable (helper missing)'
      : recording
        ? `Recording · ${clock(elapsed)}${v.detected?.title ? ` · ${v.detected.title}` : ''}`
        : v.phase === 'detected'
          ? `Meeting detected · ${v.detected?.title ?? ''}`.trim()
          : v.phase === 'processing'
            ? 'Summarizing…'
            : 'Not recording';
  const items: TrayItem[] = [
    { label: status, enabled: false },
    { label: '', separator: true },
  ];
  if (recording) items.push({ label: 'Stop Recording', action: 'stop' });
  else
    items.push({
      label: v.phase === 'detected' ? 'Record This Meeting' : 'Start Recording',
      action: 'start',
      enabled: v.phase !== 'processing' && v.helper !== 'missing',
    });
  if (v.phase === 'detected')
    items.push({ label: 'Not Now', action: 'dismiss' });
  items.push(
    { label: '', separator: true },
    { label: 'Open qrtz', action: 'open' },
    { label: 'Quit qrtz', action: 'quit' },
  );
  return {
    icon: recording ? ('rec' as const) : ('idle' as const),
    title: recording ? clock(elapsed) : '',
    tooltip: `qrtz · ${status}`,
    items,
  };
}
