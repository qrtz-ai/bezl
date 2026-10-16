# qrtz

qrtz desktop app for macOS, built with Electron + Electron Forge (Vite + TypeScript).

## Requirements

- Node.js >= 22.13 (required by Electron Forge 8)
- npm >= 10.9

## Development

```sh
npm install
npm start          # run the app with hot reload
npm run lint       # oxlint + oxfmt check
npm run lint:fix   # auto-fix lint/format
npm run typecheck  # tsc --noEmit
```

## Build & package

```sh
npm run package    # packaged app in out/ for the current platform
npm run make       # distributables in out/make/
```

On macOS, `npm run make` produces a `.dmg` and a `.zip` of `qrtz.app`.
To build only for mac explicitly: `npx electron-forge make --platform=darwin`.

> Note: DMGs can only be built on macOS. On Linux CI/dev boxes, `npm run package`
> works but the DMG maker is skipped. Code signing / notarization is not set up yet.

## Security defaults

- `contextIsolation: true`, `nodeIntegration: false`, `sandbox: true`
- Preload script (`src/preload.ts`) exposes a minimal API via `contextBridge`
- Strict Content-Security-Policy in `index.html`
- Electron fuses hardened in `forge.config.mts` (no RunAsNode, ASAR integrity, etc.)
