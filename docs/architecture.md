# Architecture

## System overview (as built)

```
┌──────────── Electron main (apps/desktop/src/main) ────────────┐
│ IPC router (core/ipc): envelope → channel/version → capability │
│   → zod request → handler → zod response                       │
│ NoteService: session state machine, helper events → worker     │
│ Supervisor ×2 (backoff, crash-loop breaker, heartbeat)          │
│ ConfigStore · tray · mini HUD                                   │
│ auto-update scaffold (off until signed)                         │
└───────┬──────────────────────────┬─────────────────────────────┘
        │ stdio NDJSON (zod)       │ utilityProcess RPC
┌───────▼────────┐        ┌────────▼───────────────────────────────┐
│ qrtz-helper    │        │ worker (src/worker.ts + worker/*)       │
│ Swift, macOS   │        │ node:sqlite + FTS5 · NoteRepo           │
│ AX detection,  │        │ finalize/recover/summary (LLM or        │
│ Core Audio tap │        │ extractive) · chat retrieval (FTS +     │
│ / SCK, LocalVQE│        │ on-device MiniLM embeddings, hybrid)    │
│ AEC, Parakeet  │        │ export · retention                      │
│ streaming ASR  │        └─────────────────────────────────────────┘
│ (TS mock off-  │
│  Mac)          │        Renderer (sandboxed, CSP): shell + sub apps
└────────────────┘        (note, settings, gallery*) via @qrtz/ui
```

- **Packages:**
  - `@qrtz/core` holds the contracts and pure logic: the IPC contract and events, capabilities, the helper protocol and mock, the supervisor, the config schema, and the note domain (transcript assembly, speaker fusion, summary, chat, embeddings ranking, repo, migrations).
  - `@qrtz/ui` holds the design system.
  - `packages/features/*` are the sub apps. Each has a static manifest with capabilities and a lazy `load()`.
- **Boundaries:**
  - `scripts/check-boundaries.mjs`: features import only core and ui. Renderer code can't import `electron` or `node:*`.
  - `scripts/check-swift.mjs`: catches duplicate Swift types and unbalanced `#if` blocks.
- **Capabilities:**
  - Each sub app manifest lists capabilities, for example `note.read`, `note.record`, `note.write`, and `config.write`.
  - Main enforces them per call and per event subscription.
  - Narrow channels exist so features don't need broad grants. For example, `note:set-auto-record` needs only `note.record`, not `config.write`.
- **Data:**
  - SQLite in userData holds meetings, segments (FTS5), summaries, meetings_fts and segment_vectors. Migrations live in core and are append-only.
  - Recordings are WAV files under `userData/recordings`, served to the renderer only through `qrtz-media://rec/<uuid>.wav`.
- **Fail-soft paths:**
  - Helper crash: the live transcript and WAV are salvaged and the header repaired.
  - Helper missing: shown in the UI and recording is disabled.
  - LLM failure: extractive fallback, with retry.
  - Embedder unavailable: full-text retrieval only.
  - Worker restarts: requests fail fast and the UI shows retry.
- **Dev/test-only hooks:**
  - The variables are listed in `DEV_ONLY_ENV` in `main/index.ts` and stripped in packaged builds.
  - `QRTZ_MOCK_HELPER`, `QRTZ_USER_DATA` and `QRTZ_EMBEDDINGS` are allowed for headless and diagnostic use.

## Research and rationale

## What we looked at

- **Package manager / workspaces.** pnpm workspaces are the default choice for most JS
  monorepos. However, Electron Forge still expects a hoisted, app-local `node_modules`
  and needs `node-linker=hoisted` under pnpm (electron/forge#4188 is still open in 2026).
  npm workspaces hoist by default and work with Forge out of the box.
- **Task runners (Turborepo, Nx).** Both are useful once there are many packages with
  their own build outputs and CI caching matters. With one app and source-only
  internal packages, there is nothing to cache besides `tsc -b`, which is already
  incremental. Nx also brings generators and module-boundary lint rules, but at the
  cost of a lot of config.
- **Electron + Vite with internal packages.** The pattern that avoids stale pre-bundled
  workspace deps is to consume workspace packages as TypeScript source and let Vite
  compile them. Forge 8 bundles main and preload fully, so the packaged asar
  contains only `.vite/` output and no symlinked workspace `node_modules`.
- **How large apps do feature modules.**
  - **VS Code:** extensions declare a static manifest (`package.json` contributes:
    commands, views) that the shell reads without loading code, and they activate
    lazily. They talk to the host only through a versioned API.
  - **Raycast:** each extension is a separate package with a manifest listing commands
    (name, icon, mode). The app renders them in its UI and builds each command's React
    tree on demand, using Raycast's own `@raycast/api` component library rather than
    raw DOM. That is the same split as our core and ui.
  - **Linear-style apps:** a single SPA organised into feature folders and packages
    around a shared design system and a central route and command registry. They're
    compiled in, not dynamic plugins.

## What we chose

| Decision          | Choice                                                                                    | Why                                                                                                                                                              |
| ----------------- | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Workspaces        | npm workspaces                                                                            | Works with Forge packaging today. No extra tooling.                                                                                                              |
| Task runner       | Plain root scripts                                                                        | One app. `tsc -b` already handles ordering and incrementality. Turborepo can be added later without restructuring.                                               |
| Internal packages | Source-only TS (`exports` → `src/index.ts`)                                               | No per-package build. HMR across packages. Vite bundles everything.                                                                                              |
| Type safety       | Project references and `tsconfig.base.json`                                               | Per-package type boundaries and ordered builds.                                                                                                                  |
| Sub-app model     | Static manifest and lazy `load()` (VS Code and Raycast style), compiled in (Linear style) | The sidebar renders without loading features, and each feature is its own chunk. Compiled-in avoids the security and versioning cost of runtime plugins for now. |
| Registration      | Explicit list in `apps/desktop/src/shell/subapps.ts`                                      | One obvious place, type-checked. No magic globbing across packages.                                                                                              |
| Boundaries        | `scripts/check-boundaries.mjs`, run by `npm run lint`                                     | Features depend only on core and ui. Checked in both package.json and import statements. Lighter than Nx and works with oxlint.                                  |
| IPC               | One `IpcContract` type in core, allow-listed in preload                                   | Main, preload and renderer can't drift apart. The renderer gets no raw `ipcRenderer`.                                                                            |
| Design system     | `@qrtz/ui` with tokens, a theme context, and primitives                                   | Every sub app looks consistent. Swappable for CSS variables or vanilla-extract later.                                                                            |

## Possible later steps

- Turborepo, once there are multiple apps or CI caching starts to matter.
- Isolating sub apps in their own `WebContentsView` (capabilities are enforced in main today, but every sub app shares one renderer).
- Bundling `@huggingface/transformers` and onnxruntime-node into the packaged app (asar-unpacked native addon). Today the packaged build falls back to full-text chat retrieval.
- Moving sub apps to runtime-loaded plugins (separate bundles plus a versioned API), if
  third parties ever need to ship them.
