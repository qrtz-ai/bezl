# Contributing to Qrtz

Short, binding practices for everyone (people and automation) working in this repo.

## Landing changes

- A change lands once Linux CI is green on the **branch head** and the tech lead has cleared the commit.
- CI on `main` is a backstop, not the gate.
- Keep CI light. Don't make CI a blocker for other work.

## macOS

- macOS CI runs nightly on `main` and before each release.
- If a macOS run on `main` goes red: **revert first, debug after.**
- An exact-head macOS run is required only for changes to `release.yml`,
  signing/attestation scripts, and entitlements.

## High-churn paths

These need extra care, a narrow diff, and explicit tech-lead review:

- `.github/workflows/*`
- `scripts/release/*`
- `scripts/check-*`
- entitlements files
- `mac-gate.sh`
- Dash enrollment
- `native/`

## Security work

- Post the approach on the ticket **before** writing any code.
- Changes to root security checks need tech-lead sign-off.

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

- Never force-push.
- Never bump versions outside the release process.
- Never commit secrets, credentials or personal paths.
- Don't delete branches or files without explicit approval.

## Tickets

1. Move the ticket to **In Progress** when work starts.
2. Add one comment with the branch and commit hash.
3. Move it to **In Review**.
