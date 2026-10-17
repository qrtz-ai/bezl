import { Menu, Tray, nativeImage, type NativeImage } from 'electron';
import type { SessionView } from '@qrtz/core';
import { trayState, type TrayAction } from './tray-model';
import {
  TRAY_IDLE_1X,
  TRAY_IDLE_2X,
  TRAY_REC_1X,
  TRAY_REC_2X,
} from './tray-icons';

function icon(one: string, two: string): NativeImage {
  const img = nativeImage.createFromDataURL(one);
  img.addRepresentation({ scaleFactor: 2, dataURL: two });
  img.setTemplateImage(true); // macOS tints template images for light/dark menu bars
  return img;
}

/** Menu bar (tray) item: recording state + elapsed time, start/stop, open app. */
export function createTray(o: {
  getView: () => SessionView;
  onAction: (a: TrayAction) => void;
}) {
  const icons = {
    idle: icon(TRAY_IDLE_1X, TRAY_IDLE_2X),
    rec: icon(TRAY_REC_1X, TRAY_REC_2X),
  };
  const tray = new Tray(icons.idle);
  let last = '';
  const render = () => {
    const s = trayState(o.getView());
    const key = JSON.stringify(s);
    if (key === last) return;
    last = key;
    tray.setImage(icons[s.icon]);
    tray.setToolTip(s.tooltip);
    if (process.platform === 'darwin')
      tray.setTitle(s.title, { fontType: 'monospacedDigit' });
    tray.setContextMenu(
      Menu.buildFromTemplate(
        s.items.map((i) =>
          i.separator
            ? { type: 'separator' as const }
            : {
                label: i.label,
                enabled: i.enabled ?? true,
                ...(i.action ? { click: () => o.onAction(i.action!) } : {}),
              },
        ),
      ),
    );
  };
  render();
  // Elapsed time in the menu bar ticks once a second while recording.
  const timer = setInterval(() => {
    if (['starting', 'recording', 'reconnecting'].includes(o.getView().phase))
      render();
  }, 1000);
  return {
    update: render,
    destroy() {
      clearInterval(timer);
      tray.destroy();
    },
  };
}
