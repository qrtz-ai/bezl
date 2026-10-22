# Contributing to Qrtz

Short, binding practices for everyone (people and automation) working in this repo.

## Review and landing

Risk-tiered review, approved by Jared on 2026-10-22 (QRT-342). Review effort goes where a
mistake can't be undone or grants more access than intended; everything else lands fast.

### Low risk: land on green Linux CI, no pre-merge clearance

- Frontend (`packages/ui`, app views, stories), tests, docs, copy, screenshot baselines, and
  Rust crates that don't touch native code or permissions.
- Lands once Linux CI is green on the branch head. No clearance request, no re-clearing
  after a rebase, no hashes posted in the room.
- Every commit names its Linear ticket. The owner lands their own work.
- Bozniak may review after landing.

### Gated: Bozniak pre-clears before landing

- Signing, release, and the workflows that run them (`.github/`). Workflow changes also
  need Jared's review (CODEOWNERS).
- Entitlements and macOS permissions.
- Anything that starts a process or decides what it can reach: process spawning,
  confinement, the module host, updater, downloader, and IPC between processes.
- Version bumps (only through the release process).
- Module manifest format changes.

The path list is kept in [docs/ci.md](docs/ci.md#gated-paths-bozniak-pre-clears).

### macOS

- Push and PR runs are Linux only. A nightly macOS run on `main` (3 AM PT) is the safety net;
  macOS also runs for releases and on demand (`gh workflow run ci.yml --ref <branch> -f macos=true`).
- An exact-head `macos=true` run is required only for gated native changes.
- A rebase that only picks up commits touching other files keeps the earlier green run.

### Breakage

- If `main` (Linux or nightly macOS) goes red: fix forward or revert immediately. Revert
  first if the fix isn't obvious.
- CI on `main` is a backstop. Keep CI light; don't make it a blocker for other work.

## Security work

- Post the approach on the ticket **before** writing any code.
- Changes to root security checks need Bozniak's sign-off.

## Modules are self-contained

- Each package keeps its own scripts, dependencies and config in its own
  folder and `package.json`.
- Never add a module's scripts, deps or config to the repo root.

## Releases and signing

- Signing runs in GitHub Actions through a protected `release` environment,
  usable only from `release.yml` on `main` and release tags.
- A maintainer is the required approver. Self-review is off and admins cannot bypass.
- No bot or automation approves releases or touches signing secrets.
  Only a maintainer uploads secrets.
- Workflow changes require CODEOWNERS review.

Guardrails:

- **Environment-only secrets.** Signing secrets live only in the `release`
  environment, never as repo or org secrets.
- **Triggers.** `release.yml` runs only on `workflow_dispatch` or a tag push,
  never on `pull_request`, `pull_request_target` or `workflow_run`.
- **Build job without secrets.** No environment, no secrets, minimal
  permissions (`contents: read`, `id-token: write`, `attestations: write`).
  It produces the bundle and its attestation.
- **Sign job verifies first.** A separate job with `environment: release`
  verifies the attestation (signer workflow and `main` source ref) before it
  touches any key. No caches; temporary keychain deleted in an `always()` step.
- **Pinned actions.** Every action is pinned to a full commit SHA, in all workflows.
- **No secrets for PR or branch workflows.** `ci.yml` stays `contents: read`.
- **Visible diff.** The sign job prints the `.github/` and `scripts/release/`
  diff since the last release tag so the approver sees what they approve.

## Hygiene

- Never force-push. Never move a pushed tag.
- Never stage `native/qrtz-helper/Package.resolved`.
- Commits are authored and committed as `Jared Grimes <jlgrimes@users.noreply.github.com>`.
- Never bump versions outside the release process.
- Never commit secrets, credentials or personal paths.
- Don't delete branches or files without explicit approval.

## Tickets

1. Move the ticket to **In Progress** when work starts.
2. Add one comment with the branch and commit hash.
3. Move it to **In Review**.
