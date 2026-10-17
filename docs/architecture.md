# Architecture: research and rationale

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
- Per-sub-app permissions in the manifest. The preload would scope IPC per feature.
- Moving sub apps to runtime-loaded plugins (separate bundles plus a versioned API), if
  third parties ever need to ship them.
