# LOOP.md: rules for every iteration (Ralph-style)

Each iteration is one pass of: **read, pick, build, verify, commit, push, log.**

1. **Read** the ticket you're working on and the last entries in `PROGRESS.md`.
2. **Pick** one ticket. Tasks are small (≤ ~30 min). If one is bigger, split it into
   smaller tickets first, then pick the first piece.
3. **Build** only that task. Don't refactor unrelated code. Respect the boundary rules
   (`scripts/check-boundaries.mjs`).
4. **Verify**, with all of these green from the repo root using bun ≥ 1.4
   ```sh
   bun run lint && bun run typecheck && bun run test && bun run check:rust
   ```
   Also run `bun run e2e` when the renderer, the shell or the IPC contract changed.
   A task is done only when its ticket's **acceptance criteria** hold.
5. **Commit** with a message that names its ticket (`QRT-<n>`), authored as
   `Jared Grimes <jlgrimes@users.noreply.github.com>` (see `AGENTS.md`), and push to `main`.
6. **Log**: put the commit SHA on the ticket and append to PROGRESS.md: date and time (PT),
   ticket id, commit SHA, what was verified, and anything for the Mac.
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
- Then run `QRTZ_REQUIRE_SWIFT=1 bun run test:helper`. The script finds the /tmp toolchain on its own, fails if it is missing or if 0 XCTests ran, and prints "N Swift tests passed".
- After that build, `packages/core/src/helper/swift-helper.test.ts` drives the real binary over stdio (it is skipped when there's no build).
- Protocol changes must update `native/qrtz-helper/Fixtures/protocol/*.jsonl`. Both sides' tests enforce full coverage.
