import { app, ipcMain } from 'electron';
import type { IpcChannel, IpcRequest, IpcResponse } from '@qrtz/core';

function handle<C extends IpcChannel>(
  channel: C,
  fn: (req: IpcRequest<C>) => IpcResponse<C> | Promise<IpcResponse<C>>,
) {
  ipcMain.handle(channel, (_e, req: IpcRequest<C>) => fn(req));
}

export function registerIpc() {
  handle('app:get-info', () => ({
    version: app.getVersion(),
    platform: process.platform,
  }));
}
