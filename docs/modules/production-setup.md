# Production setup: what's done, and Jared's checklist

v1 ships one signed, notarized qrtz.app and its package hosting: the `index` GitHub release (QRT-297), the signed
index and `qrtz install …` over the network. echo, nano and ctrl are built in (the bundled `qrtz-helper`), and so is snap
(`qrtz-snap`, ADR 0159). Part A is the signed build. Part B is the hosting.

## What's done (box side, 2026-10-18)

- **The `qrtz` command is in every build** as `qrtz.app/Contents/MacOS/qrtz`, an `externalBin` that
  `bun run build:helper` builds. The app's own executable is now `Contents/MacOS/qrtz-app`. The bundle, its identifier
  and its Privacy entries are unchanged, and the login item is fixed on the first launch
  ([ADR 0162](../adr/0162-the-qrtz-command-ships-as-contents-macos-qrtz.md)). `qrtz link` puts `~/.local/bin/qrtz`
  on your PATH as a symlink into the bundle. Until `root1Sha256` is pinned it refuses every install, as designed.
- **Every executable in `Contents/MacOS` gets the right signature.** `tauri build` gives every bundled executable the
  app's entitlements, so `scripts/release/sign-mac.sh` signs each one again: `qrtz` with none and `qrtz-echo` as
  `ai.qrtz.echo` with its microphone entitlement (ADR 0174: echo is bundled; snap is a package). Then it reseals and checks the app, notarizes and staples it, and makes, notarizes and
  staples the `.dmg` (A4). The list it signs is checked against `docs/modules/confinement-map.json` by a test.
- **Package manifests**: `modules/<name>/qrtz.json` for nano, ctrl and snap (ADR 0174: echo ships inside qrtz.app
  and `inference.voice` is cut from v1).
- **`scripts/modules/release-keys.sh`**: the whole key ceremony in one command (offline only). Root 1 has threshold 2,
  so it is signed by both root keys and a key rotation needs both.
- **`scripts/modules/publish.sh`**: build, verify, upload, and a live check (QRT-297: the upload is
  `scripts/release/gh-store.sh`, assets on the fixed `index` GitHub release; no Cloudflare). `--out-only` (or
  `--no-upload`) builds and verifies and writes `~/qrtz-publish-out.upload.tsv` with no upload and no token;
  `--timestamp` is the re-sign. The host never holds model files: the index pins each one's Hugging Face URL and hash
  and the app downloads it from there (ADR 0171).
- **`.github/workflows/release.yml`** (QRT-297): releases and the daily timestamp re-sign run in GitHub Actions. See
  [Releases on GitHub](#releases-on-github-qrt-297) below.
- **Proven end to end on the box** with throwaway keys (deleted afterwards): `release-keys.sh` (threshold 2: root 1 signed by both root keys) → `publish.sh --no-upload` with the five real pinned
  model sets (1.7 GB, every file matching its pin) and the repo's own `qrtz.json` for echo, nano, ctrl, snap and
  `inference.voice` (stub executables) → index 1 with 10 packages, 79 blobs verified → `qrtz-publish timestamp`
  (timestamp 2) → `pack echo`. Root 1 with one of its two signatures removed is refused by both `qrtz-publish verify`
  and the client ("signature threshold not met"). The CLI's own code (`pm_cli::run` over `Packages`, throwaway pin,
  accepting code checks) then sideloaded echo (3 packages), `install --frozen --offline` was a no-op,
  `install nano` over a loopback Range server fetched 844.5 MB, `install ctrl snap` fetched `model.laya` (483.9 MB),
  `engine echo model.nemotron` and back, `remove nano`, `prune` (4 dirs, 844.5 MB), and `--frozen --offline` again.
  The real `qrtz` binary's `link`/`unlink` made and removed `~/.local/bin/qrtz` (under a scratch HOME), and it refuses
  installs with `no root is pinned`, as it must until `root1Sha256` is set.
- **Model licenses checked.** All five may be mirrored, with notices. Those are in `THIRD_PARTY_NOTICES.md`:
  Parakeet TDT v2 is CC-BY-4.0, Nemotron streaming is the NVIDIA Open Model License, the Nemotron 3 diarizer is
  OpenMDW-1.1, and LocalVQE and Laya are Apache-2.0.

echo ships inside qrtz.app and works with no index (ADR 0174; switching it on downloads Parakeet from Hugging Face,
hash-pinned in qrtz). nano, ctrl and snap install only once they are published; each one's executable is notarized by publish.sh
and checked offline by the installer (ADR 0170).

## Part A: the signed v1 build (needed for launch)

**A1. Apple Developer Program.** Done (2026-10-18). Team ID **`MR26CCZ496`**.

**A2. Developer ID Application certificate.** Done (2026-10-18): `Developer ID Application: Jared Grimes
(MR26CCZ496)` (SHA-1 `E689C10899E1DD9E9E582035C7AD3E563202D2FA`), in your login keychain. Both are public and are
now the repo's release defaults: the bundled lock and manifests pin team `MR26CCZ496` in every designated
requirement (`qrtz_modules::manifest::RELEASE_TEAM_ID`), and the release scripts default to that identity
(`QRTZ_SIGN_ID` or `APPLE_SIGNING_IDENTITY`, and `QRTZ_TEAM_ID`, still override it). `offline_graph`'s
`every_release_path_pins_jareds_developer_id_team` fails if a release path names another team. The certificate's
private key never leaves your Mac.

**A3. Notarization credentials (once).** Make an app-specific password at <https://account.apple.com> → **Sign-In
and Security → App-Specific Passwords** (name it `qrtz-notary`), then save it as a notarytool profile in your
keychain:

```sh
xcrun notarytool store-credentials qrtz-notary --apple-id "<your Apple ID email>" --team-id MR26CCZ496
# it asks for the app-specific password: paste it into that prompt only, never into chat or a file. Check it with:
xcrun notarytool history --keychain-profile qrtz-notary
```

**A4. The signed build, notarized and stapled: one command.** From a clean checkout of `main` on your Mac:

```sh
git pull && bun install --frozen-lockfile
bun run release:mac
```

`bun run release:mac` is `scripts/release/sign-notarize.sh`. With nothing exported it uses `Developer ID
Application: Jared Grimes (MR26CCZ496)`, team `MR26CCZ496` and the `qrtz-notary` profile. It runs `bun run make` with
`APPLE_ID`/`APPLE_PASSWORD`/`APPLE_API_KEY` unset (so tauri build signs but doesn't notarize too early), then
`scripts/release/sign-mac.sh` (re-signs `Contents/MacOS/*`, reseals, checks, notarizes, staples, makes the `.dmg`),
then checks the team and hardened runtime on every executable, the staples and Gatekeeper. It ends with:

```
== release
dmg:        …/release/bundle/macos/qrtz_<version>_aarch64.dmg
sha256:     <64 hex>
gatekeeper: …: accepted source=Notarized Developer ID …
```

`bun run release:mac -- --no-notarize` signs and checks only (nothing goes to Apple), and
`bun run release:mac -- --app <qrtz.app>` skips the build. The manual steps it wraps are in docs/release.md.

Every step it runs must succeed, and it stops at the first failure. The checks are: `codesign --verify --strict
--deep`; for each of `qrtz-helper`, `qrtz` and `qrtz-snap`, its designated requirement with your team, the hardened
runtime and the exact entitlements; `spctl` on the app ("source=Notarized Developer ID") and on the `.dmg`; `stapler
validate` on both; and `qrtz --help` from the signed app. It writes `qrtz_<version>_aarch64.dmg` next to the app.
Ignore the `.dmg` that `bun run make` built in `bundle/dmg/`, because its signatures are the bundler's.

If `notarytool` reports `Invalid`, get the reason with
`xcrun notarytool log <submission id> --keychain-profile qrtz-notary` and **send us** that log. It names the file and
the problem. Then open the `.dmg` on a second Mac (or a new user account), drag qrtz to Applications and launch it. It
should open with no Gatekeeper warning. Turn on snap and press ⌥⌘← on a window. It should snap to the left half
with no new Accessibility prompt, because it uses qrtz's grant. **Send us:** the script's last 20 lines, or just
"all passed". The
`QRTZ_SPEECH_*` variables in docs/release.md are optional. Without them the build uses the helper's recognizer, as
today.

## Part B: package hosting (v1)

The keys (B4) and the root pin (B5) are done. The hosting moved from Cloudflare R2 to GitHub Releases on 2026-10-20
(QRT-297); the old Cloudflare zone, R2 bucket and B2 mirror steps are gone. The Dash worker
(`services/dash-worker`, `dash.qrtz.ai`) still runs on Cloudflare and is not part of this.

**B4. Keys, offline.** Done 2026-10-18 (`scripts/modules/release-keys.sh`; root 1 has threshold 2, both root keys).
`root1Sha256 = 02dbb15f7927e343c5747a4d085a564775a49aecec2f854c7d028c4935afaa7d`, root 1 expires 2027-10-18. Never
send any `*.key` file, and never put a root key in GitHub.

**B5. Pin the root.** Done 2026-10-18. `root1Sha256` is set in `crates/qrtz-app/src/module_hosts.json` and the public
`1.root.json` and `.sig` are checked in at `crates/qrtz-modules/roots/`. The first publish to GitHub uploads them as
the assets `v1.root.1.root.json` (+ `.sig`).

**B6. Back up** `~/.qrtz-keys` to an encrypted disk. The `index` release is now the record of everything published
(`scripts/ci/hydrate-out.sh` rebuilds a publish dir from it).

## Releases on GitHub (QRT-297)

**Where things live.** Each release is a tag. `app-v<x.y.z>` is qrtz.app. `nano-v<x.y.z>`, `ctrl-v<x.y.z>` and
`snap-v<x.y.z>` are packages. Each tag gets its own GitHub release with the signed dmg or package attached, for people.
The app itself reads only the fixed `index` release, through direct download URLs such as
`https://github.com/qrtz-ai/qrtz/releases/download/index/v1.timestamp.json` (never `api.github.com`, which is
rate-limited). Its assets are the signed root, timestamp and index, every package blob (named by its sha256) and
qrtz.app's update feed (`v1.app.latest.json`) and archives. A layout path `v1/a/b` is the asset `v1.a.b`. The app
follows GitHub's redirect only to `objects.githubusercontent.com` and `release-assets.githubusercontent.com`
(`module_hosts.json` `redirectHosts`), and checks every byte against the signed index or the update key as before.

**What runs.** `.github/workflows/release.yml`:

1. `plan` reads the tag, checks the tagged commit's version matches, and checks that commit is on main.
2. `build` compiles the app or the package on a macOS runner with **no secrets** and a read-only token.
3. `sign` runs only for a commit on main, in the `release` environment, so **it waits for your approval**. It
   re-signs everything inside out with the Developer ID, notarizes and staples it, signs the update archive and
   feed (app) or the package index and timestamp (package), and uploads to GitHub Releases. Its token has
   `contents: write` and nothing else.
4. `timestamp` runs every day at 16:00 UTC (9 AM PDT) in the separate `release-timestamp` environment, which holds
   only the timestamp key. It re-signs the index timestamp (clients refuse one older than 8 days).
5. `feed-refresh` (run by hand, `tag=feed-refresh`) re-signs qrtz.app's update feed, which expires 30 days after it
   was signed.

To release: bump the version (`node scripts/app/version.mjs bump`, or `node scripts/modules/version.mjs bump <m>`),
push to main, then `node scripts/app/version.mjs tag --push` (it tags `app-v<x.y.z>`), or for a package
`git tag <m>-v<x.y.z> && git push origin <m>-v<x.y.z>`. Approve the `sign` job when
GitHub asks.

### One-time setup (on your Mac)

**1. The environments.** In GitHub: qrtz-ai/qrtz → **Settings** → **Environments**.

- **New environment** `release`:
  - Tick **Required reviewers** and add yourself (`jlgrimes`). Leave **Prevent self-review** unticked (you are the
    only reviewer).
  - **Deployment branches and tags** → **Selected branches and tags** → add the branch rule `main` and the tag rules
    `app-v*`, `nano-v*`, `ctrl-v*`, `snap-v*`.
- **New environment** `release-timestamp`:
  - No required reviewer (it runs daily on its own).
  - **Deployment branches and tags** → **Selected branches and tags** → add the branch rule `main` only.

Or from Terminal (`gh` signed in as you; adds the reviewer and the branch rule; add the tag rules in the web page):

```sh
id="$(gh api user --jq .id)"
gh api -X PUT repos/qrtz-ai/qrtz/environments/release \
  -F "reviewers[][type]=User" -F "reviewers[][id]=$id" \
  -F "deployment_branch_policy[protected_branches]=false" -F "deployment_branch_policy[custom_branch_policies]=true"
for p in main; do gh api -X POST repos/qrtz-ai/qrtz/environments/release/deployment-branch-policies -f name="$p" -f type=branch; done
for p in 'app-v*' 'nano-v*' 'ctrl-v*' 'snap-v*'; do gh api -X POST repos/qrtz-ai/qrtz/environments/release/deployment-branch-policies -f name="$p" -f type=tag; done
gh api -X PUT repos/qrtz-ai/qrtz/environments/release-timestamp \
  -F "deployment_branch_policy[protected_branches]=false" -F "deployment_branch_policy[custom_branch_policies]=true"
gh api -X POST repos/qrtz-ai/qrtz/environments/release-timestamp/deployment-branch-policies -f name=main -f type=branch
```

**2. The secrets.** Each command asks for the value (paste it at the prompt, or pipe it in as shown). Never paste a
value into chat, an issue or a file in the repo. These run on your Mac only.

```sh
# Developer ID: export only "Developer ID Application: Jared Grimes (MR26CCZ496)" from Keychain Access as a .p12
# with a new password, then:
base64 -i ~/Desktop/DeveloperID.p12 | gh secret set APPLE_CERT_P12_BASE64 --env release --repo qrtz-ai/qrtz
gh secret set APPLE_CERT_PASSWORD --env release --repo qrtz-ai/qrtz          # the .p12 password
rm -P ~/Desktop/DeveloperID.p12
# Notarization (your Apple ID and an app-specific password from account.apple.com → App-Specific Passwords):
gh secret set APPLE_ID --env release --repo qrtz-ai/qrtz                     # your Apple ID email
gh secret set APPLE_TEAM_ID --env release --repo qrtz-ai/qrtz --body MR26CCZ496
gh secret set APPLE_APP_PASSWORD --env release --repo qrtz-ai/qrtz          # the app-specific password
# qrtz.app's update key (minisign, made by `scripts/ci/setup-secrets.sh app-update` if you don't have one yet):
tr -d '[:space:]' < ~/.qrtz-keys/app-update.key | gh secret set TAURI_SIGNING_PRIVATE_KEY --env release --repo qrtz-ai/qrtz
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env release --repo qrtz-ai/qrtz   # press Enter: the key has no password
# The package index's online keys (never a root key):
tr -d '[:space:]' < ~/.qrtz-keys/index.key | gh secret set QRTZ_INDEX_KEY --env release --repo qrtz-ai/qrtz
tr -d '[:space:]' < ~/.qrtz-keys/timestamp.key | gh secret set QRTZ_TIMESTAMP_KEY --env release --repo qrtz-ai/qrtz
tr -d '[:space:]' < ~/.qrtz-keys/timestamp.key | gh secret set QRTZ_TIMESTAMP_KEY --env release-timestamp --repo qrtz-ai/qrtz
# Check (names only):
gh secret list --env release --repo qrtz-ai/qrtz && gh secret list --env release-timestamp --repo qrtz-ai/qrtz
```

`scripts/ci/setup-secrets.sh` does the same from your keychain and key files in one go (it exports the Developer ID
identity for you).

**3. Notifications.** On the repo page: **Watch** → **Custom** → tick **Releases**, so every published release
emails you. A release you didn't start is the signal to delete it and rotate the keys.

**4. The update public key.** If `crates/qrtz-app/src/app_update_key.pub` still holds no key, run
`scripts/ci/setup-secrets.sh app-update` once and commit the `.pub` it writes. The `sign` job refuses to publish an
app update without it.

**5. The first publish.** Push a package tag (e.g. `git tag nano-v<version in modules/nano/qrtz.json>` and
`git push origin nano-v…`), approve the run, and check
`curl -sL https://github.com/qrtz-ai/qrtz/releases/download/index/v1.timestamp.json`. Then the app: `git tag
v<version in tauri.conf.json>` and push it.

**Cleanup in Cloudflare (when you're ready, by hand).** Nothing in qrtz reads R2 any more. You can delete the
`qrtz-modules` bucket, its `modules.qrtz.ai` custom domain and the R2 API token, and the `qrtz-r2` keychain item on
your Mac. Keep the `qrtz.ai` zone and everything the Dash worker uses.
