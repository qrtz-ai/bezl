# Module publishing on GitHub (ADR 0218, 0.3.0)

Module packages, the signed index and the timestamp now live as assets of one GitHub release of qrtz-ai/qrtz,
`modules-index`, instead of the Cloudflare R2 bucket behind modules.qrtz.ai. Nothing runs on Jared's Mac any more.

| What                                                                            | Where                                                                                                                                                                                                        |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Publish (build, notarize, sign index + timestamp, upload, tag `<m>-v<version>`) | `.github/workflows/modules.yml`, Run workflow on main, environment `release` (Jared approves every run)                                                                                                      |
| Daily timestamp re-sign (16:00 UTC; 7-day validity, clients refuse after 8)     | `.github/workflows/modules-timestamp.yml`, environment `modules-timestamp` (no reviewer)                                                                                                                     |
| Client host                                                                     | `crates/qrtz-app/src/module_hosts.json`: primary `https://github.com/qrtz-ai/qrtz/releases/download/modules-index`, mirror `https://modules.qrtz.ai` (fallback until R2 is shut down). Root 1 pin unchanged. |

## Asset names

A release has no directories, so each layout path is one flat name: every `/` becomes `.`.

| Layout path                    | Asset                          |
| ------------------------------ | ------------------------------ |
| `v1/root/<n>.root.json[.sig]`  | `v1.root.<n>.root.json[.sig]`  |
| `v1/timestamp.json[.sig]`      | `v1.timestamp.json[.sig]`      |
| `v1/index/<sha256>.json[.sig]` | `v1.index.<sha256>.json[.sig]` |
| `v1/blobs/sha256/<sha256>`     | `v1.blobs.sha256.<sha256>`     |

The client (`module_hosting::flat_asset_name`) and the scripts (`scripts/modules/gh-assets.mjs`) use the same map.
Immutable objects are uploaded once and never replaced; only the timestamp pair is replaced (`--clobber`). The
client follows GitHub's redirect only to `github.com`, `objects.githubusercontent.com` and
`release-assets.githubusercontent.com` over HTTPS. Every byte is still checked against the signed index.

The first `modules.yml` run seeds the release from modules.qrtz.ai (root, timestamp, index and every blob the
index names), verifies it against root 1, then publishes on top. The release is created with `--latest=false` so it
never becomes the app's "latest" release.

## Jared's steps (once, on your Mac; never print a key)

1. Index key into the `release` environment:
   `gh secret set QRTZ_INDEX_KEY --env release -R qrtz-ai/qrtz < ~/.qrtz-keys/index.key`
2. A full publish also signs a fresh timestamp, so the timestamp key goes into `release` too:
   `gh secret set QRTZ_TIMESTAMP_KEY --env release -R qrtz-ai/qrtz < ~/.qrtz-keys/timestamp.key`
3. Create the `modules-timestamp` environment (Settings → Environments → New, no required reviewers; deployment
   branches: `main` only), then:
   `gh secret set QRTZ_TIMESTAMP_KEY --env modules-timestamp -R qrtz-ai/qrtz < ~/.qrtz-keys/timestamp.key`
4. Publish: bump the module in a commit on main (`node scripts/modules/version.mjs bump nano`), then Actions →
   modules → Run workflow (modules: `nano`), and Approve the `release` deployment yourself.
5. Run `modules-timestamp` once by hand after step 4 to check it.
6. Tag policy: `modules.yml` runs only from main and creates `<m>-v<version>` tags itself, so the `release`
   environment's deployment rule needs no new tag pattern. If you later want module tags (`nano-v*`, `ctrl-v*`,
   `snap-v*`, or `echo-v*`) to start a publish, add them to the environment's allowed refs; that's your call.
7. When 0.3.0 is out and installed widely, unload the old Mac launchd job (`scripts/modules/timestamp-job.sh
   uninstall`) and shut R2 down; then drop `https://modules.qrtz.ai` from `mirrors` in a later release.

## Notes

- The timestamp key can only re-attest an index already signed with the index key; it can't add or change a package.
- The keys are the raw 64-hex Ed25519 seeds in `~/.qrtz-keys/*.key`; `scripts/ci/macos-signing.sh` writes them to
  0600 temp files on the runner and deletes them in an always() step.
