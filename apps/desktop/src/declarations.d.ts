/// <reference types="@electron-forge/plugin-vite/forge-vite-env" />
declare module '*.css';

/** Build-time auto-update config (vite.main.config.mts `define`). */
declare const __QRTZ_UPDATE_FEED_URL__: string;
declare const __QRTZ_AUTO_UPDATE__: boolean;
