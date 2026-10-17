# PROGRESS.md: iteration log

Newest entries at the bottom. Times are in America/Los_Angeles (PT).

## 2026-10-16: iteration 0 (setup and research)

- Research recorded in `docs/research-memo.md` and `docs/design-system.md`.
- Wrote PLAN.md, LOOP.md and this file.

## 2026-10-16 ~17:15–17:35 PT: iteration 1 (M0 architecture hardening)

- **Done:** M0.1–M0.8, M0.10 and M0.11.
  - `@qrtz/core` now has: `AppError`, a structured logger with redaction, a versioned zod IPC contract, a router, a typed client and event bus, a sub-app manifest with capabilities and permissions, a Supervisor (backoff, breaker, heartbeat), the helper JSON-lines protocol and a mock, a config schema, a `SecretStore`, and a migration runner.
  - Desktop main is split into `main/*`. The worker is a supervised utilityProcess using `node:sqlite` with FTS5. Secrets use safeStorage. There are crash hooks and security hardening.
- **M0.9:** the error boundary and crash hooks are implemented. The e2e fallback test is still pending.
- **Verified:** lint ✓, typecheck (tsc -b plus a test project) ✓, vitest 34/34 ✓, package ✓, e2e (Playwright, xvfb) ✓. The packaged binary boots and the worker starts.
- **Bugs found and fixed along the way:**
  - Vite stubbed `node:sqlite`, so it's now external.
  - zod 4 rejects duplicate discriminators; the worker protocol now uses a union.
  - `params` must be optional.
- **Note:** Playwright can't drive the fused, packaged app (the inspect fuse is off, by design). The e2e runs the production `.vite` build on the stock Electron binary.
- **Next:** M0.9 e2e test, then M1.1 (tokens).

## 2026-10-16 ~17:35–17:42 PT: iteration 2 (M1.1, M1.2)

- **Tokens:** qrtz.ai neutral-dark palette, Geist/mono type, space, radii, motion and z-index, exposed as CSS vars (`--q-*`) plus global CSS (reset, focus ring, blink/spin/fade keyframes, underline motion, reduced-motion support).
- **Primitives:** Heading (blinking `_` cursor, mono), Button (primary/secondary/ghost/danger, sm/md/lg, loading), Badge (qrtz.ai tinted style), Kbd, Spinner. Home has been restyled.
- **Verified:** lint, typecheck, 36 tests, package and e2e all green. The e2e saves a screenshot to `test-results/home.png`.
- **Next:** M1.3 (inputs).

## 2026-10-16 ~17:42–17:48 PT: iteration 3 (M2.1, M2.5, M2.6, pulled forward because they're pure TS)

- `@qrtz/core/memo`:
  - Zod domain types: Meeting, Segment, AsrEvent, ActiveSpeakerEvent, SpeakerTurn, caption and chat events.
  - `TranscriptAssembler`: partials are replaced and finals sorted; late partials and empty finals are ignored.
  - Diarization fusion: Sortformer slots are matched to AX active-speaker names by greedy one-to-one, overlap-weighted assignment with a min-share threshold. `labelSegments` applies the result.
- These live in core for now. If memo logic grows, consider a `@qrtz/memo-core` package with a boundary rule.
- **Verified:** lint, typecheck, 40 tests, package and e2e all green.
- **Next, in order:** M0.9 (e2e test for the error fallback), M1.3 → M1.7 (DS plus gallery), then M2.2 (memo DB schema and FTS), M2.3, M2.4, M2.7–M2.9.
