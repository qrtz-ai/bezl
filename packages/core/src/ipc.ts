/**
 * Typed IPC contract shared by main, preload and renderer.
 * Add a channel here and TypeScript enforces it on both ends.
 */
export interface IpcContract {
  'app:get-info': {
    request: void;
    response: { version: string; platform: string };
  };
}

export type IpcChannel = keyof IpcContract;
export type IpcRequest<C extends IpcChannel> = IpcContract[C]['request'];
export type IpcResponse<C extends IpcChannel> = IpcContract[C]['response'];

export const IPC_CHANNELS = [
  'app:get-info',
] as const satisfies readonly IpcChannel[];

export function isIpcChannel(value: unknown): value is IpcChannel {
  return (IPC_CHANNELS as readonly unknown[]).includes(value);
}

/** API exposed on `window.qrtz` by the preload script. */
export interface QrtzBridge {
  invoke<C extends IpcChannel>(
    channel: C,
    ...args: IpcRequest<C> extends void ? [] : [IpcRequest<C>]
  ): Promise<IpcResponse<C>>;
}

declare global {
  interface Window {
    /** Present only inside the qrtz shell (injected by preload). */
    qrtz?: QrtzBridge;
  }
}
