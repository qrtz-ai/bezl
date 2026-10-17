# qrtz

qrtz desktop app for macOS: an Electron shell hosting modular React "sub apps", all built on a shared design system.

## Requirements

- Node.js >= 22.13 and npm >= 10.9 (required by Electron Forge 8)

## Commands (run from the repo root)

```sh
npm install
npm start          # run the desktop app (Vite HMR for the renderer)
npm run lint       # oxlint + oxfmt check + workspace boundary check
npm run lint:fix   # auto-fix lint/format
npm run typecheck  # tsc -b across all workspace projects
npm test           # vitest unit tests
npm run e2e        # Playwright Electron smoke test (run `npm run package` first; xvfb-run on Linux)
npm run check      # lint + typecheck + test
npm run package    # packaged app in apps/desktop/out/
npm run make       # distributables in apps/desktop/out/make/ (.dmg + .zip on macOS)
```

> DMGs can only be built on macOS. On Linux, `npm run package` works but the DMG
> maker is skipped. Code signing and notarization aren't set up yet.

## Architecture

Full plan: [PLAN.md](PLAN.md) · loop rules: [LOOP.md](LOOP.md) · log: [PROGRESS.md](PROGRESS.md) · [security](docs/security.md) · [research](docs/research-memo.md)

**Process model:** main does orchestration and the validated IPC router. A supervised `utilityProcess` worker runs SQLite, migrations and heavy jobs. A supervised native Swift helper does AX scraping, audio capture and ASR, talking JSON-lines over stdio, with a TS mock on Linux and in tests.

```
apps/
  desktop/                 Electron shell: main, preload, React renderer host
packages/
  core/      @qrtz/core    sub-app contract + registry, shared types, typed IPC contract
  ui/        @qrtz/ui      React design system: tokens, theme, Text, Button
  features/
    home/     @qrtz/feature-home      example sub app
    settings/ @qrtz/feature-settings  example sub app (uses the IPC bridge)
scripts/check-boundaries.mjs          enforces dependency rules
docs/architecture.md                  research notes and rationale
```

- **npm workspaces**, no build step for internal packages: each package exports its
  TypeScript source and Vite compiles it into the shell bundle.
- **TypeScript project references** (`tsconfig.base.json` plus `tsc -b`) give per-package
  typechecking in dependency order.
- **Dependency rules** (checked by `npm run lint`):
  - `core` and `ui` depend on no other `@qrtz/*` package.
  - Features may import only `@qrtz/core` and `@qrtz/ui`, never another feature or the shell.
  - Only `apps/desktop` composes features.
- **Sub apps** export a `defineSubApp({ manifest, load })`. The shell builds a
  `SubAppRegistry` from them and renders the sidebar from `registry.list()`. Each sub
  app's view is lazy-loaded (its own chunk).
- **IPC** is typed end to end: channels are declared once in `@qrtz/core` (`IpcContract`).
  The preload exposes only `window.qrtz.invoke` for allow-listed channels, and main
  registers handlers with a typed `handle()` helper.
- **Security:** `contextIsolation`, `sandbox`, no `nodeIntegration`, a strict CSP, and
  hardened Electron fuses.

## How to add a sub app

1. Copy an example: `cp -r packages/features/home packages/features/notes`.
2. In `packages/features/notes/package.json`, set `"name": "@qrtz/feature-notes"`.
3. In `src/index.ts`, set a unique manifest:
   ```ts
   export default defineSubApp({
     manifest: { id: 'notes', name: 'Notes', icon: '✎', order: 10 },
     load: () => import('./NotesView'),
   });
   ```
   Rename the view component to match.
4. Wire it into the shell:
   - Add `"@qrtz/feature-notes": "*"` to `apps/desktop/package.json` dependencies.
   - Add `{ "path": "../../packages/features/notes" }` to `apps/desktop/tsconfig.json` references.
   - Add the same path to the root `tsconfig.json`.
   - Add `notes` to the list in `apps/desktop/src/shell/subapps.ts`.
5. Run `npm install && npm run check`. The new sub app appears in the sidebar.

If a sub app needs the main process, add a channel to `IpcContract` in
`packages/core/src/ipc.ts`, add it to `IPC_CHANNELS`, and handle it in
`apps/desktop/src/ipc.ts`.
