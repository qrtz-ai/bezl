# Agent rules for qrtz

- Keep changes small and focused, one piece of work per commit.
- Commits are authored as `Jared Grimes <jlgrimes@users.noreply.github.com>`, never a personal email
  (`git config user.email jlgrimes@users.noreply.github.com`).
- Every commit message names its Linear ticket by id, e.g. `(QRT-123)`.
- Run the tests before pushing (`cargo test`, `bun test`).
- Review is risk-tiered (see [CONTRIBUTING.md](CONTRIBUTING.md#review-and-landing)): low-risk changes land on
  green Linux CI without clearance; gated paths (signing, release, `.github/`, entitlements, permissions,
  process spawning/confinement, version bumps, manifest format) need Bozniak's pre-clearance.
- Changes to files listed in `.github/CODEOWNERS` need review from the code owner.
- Never force-push, never move pushed tags, never stage `native/qrtz-helper/Package.resolved`.
