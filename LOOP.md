# LOOP.md: rules for every iteration (Ralph-style)

Each iteration is one pass of: **read, pick, build, verify, commit, push, log.**

1. **Read** `PLAN.md` (architecture and checklist) and the last entries in `PROGRESS.md`.
2. **Pick** the first unchecked task in the lowest unfinished milestone. Tasks are
   small (≤ ~30 min). If one is bigger, split it in PLAN.md first, then pick the first piece.
3. **Build** only that task. Don't refactor unrelated code. Respect the boundary rules
   (`scripts/check-boundaries.mjs`).
4. **Verify**, with all of these green from the repo root using Node ≥ 22.13
   (on the Linux box: `export PATH=/tmp/node-v22.21.0-linux-x64/bin:$PATH`):
   ```sh
   npm run lint && npm run typecheck && npm test && npm run package
   ```
   Also run `npm run e2e` when shell/main/preload/IPC changed (it needs `xvfb-run` on Linux).
   A task is done only when its **acceptance criteria** in PLAN.md hold.
5. **Commit** with a message of the form `M<n>.<k>: <task>`. Push **straight to `main`**:
   no branches, no PRs, no GitHub Actions workflows (the token lacks workflow scope).
6. **Log**: tick the box in PLAN.md and append to PROGRESS.md: date and time (PT),
   task id, commit SHA, what was verified, and anything for the Mac.
7. Repeat until the time box ends. Before stopping, make sure `main` is green and
   PROGRESS.md says what's next.

## Standing rules

- **Swift / macOS-only code** can't be compiled on the Linux box. Write it carefully, keep
  the wire protocol in `@qrtz/core` (zod) as the source of truth, test against the
  TypeScript mock helper, and tag the task `[MAC-VERIFY]` in PROGRESS.md.
- Pure TypeScript logic gets **vitest** tests next to the code (`*.test.ts`).
- **Never** commit secrets. Keys come from Settings (safeStorage) or env vars
  (`QRTZ_*`), never from source.
- **Security defaults are non-negotiable:** contextIsolation, sandbox, no nodeIntegration,
  CSP, zod validation on every IPC boundary, and capability checks in main.
- If a check breaks and can't be fixed in the iteration, revert the change rather than push red.

## Swift helper checks (when native/ changes)

- Linux toolchain (if missing): download `swift-6.1.2-RELEASE-ubuntu24.04.tar.gz` into /tmp and extract it there.
- Then run `export PATH=/tmp/swift-6.1.2-RELEASE-ubuntu24.04/usr/bin:$PATH && npm run test:helper`.
- After that build, `packages/core/src/helper/swift-helper.test.ts` drives the real binary over stdio (it is skipped when there's no build).
- Protocol changes must update `native/qrtz-helper/Fixtures/protocol/*.jsonl`. Both sides' tests enforce full coverage.
