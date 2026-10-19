# Production setup: what's done, and Jared's checklist

v1 ships one signed, notarized qrtz.app and its package hosting: `modules.qrtz.ai`, the signed index and
`qrtz install …` over the network. echo, nano and ctrl are built in (the bundled `qrtz-helper`), and so is snap
(`qrtz-snap`, ADR 0159). Part A is the signed build. Part B is the hosting.

## What's done (box side, 2026-10-18)

- **The `qrtz` command is in every build** as `qrtz.app/Contents/MacOS/qrtz`, an `externalBin` that
  `bun run build:helper` builds. The app's own executable is now `Contents/MacOS/qrtz-app`. The bundle, its identifier
  and its Privacy entries are unchanged, and the login item is fixed on the first launch
  ([ADR 0160](../adr/0160-the-qrtz-command-ships-as-contents-macos-qrtz.md)). `qrtz link` puts `~/.local/bin/qrtz`
  on your PATH as a symlink into the bundle. Until `root1Sha256` is pinned it refuses every install, as designed.
- **Every executable in `Contents/MacOS` gets the right signature.** `tauri build` gives every bundled executable the
  app's entitlements, so `scripts/release/sign-mac.sh` signs each one again: `qrtz` with none and `qrtz-snap` as
  `ai.qrtz.snap` with none. Then it reseals and checks the app, notarizes and staples it, and makes, notarizes and
  staples the `.dmg` (A4). The list it signs is checked against `docs/modules/confinement-map.json` by a test.
- **Package manifests**: `modules/<name>/qrtz.json` for echo, nano, ctrl and snap, and
  `tools/qrtz-publish/packages/inference.voice/qrtz.json`.
- **`scripts/modules/release-keys.sh`**: the whole key ceremony in one command (offline only). Root 1 has threshold 2,
  so it is signed by both root keys and a key rotation needs both.
- **`scripts/modules/publish.sh`**: build, verify, upload to R2 and B2, and a live check, with credentials from env
  vars only. `--no-upload` builds and verifies; `--timestamp` is the re-sign.
- **`scripts/modules/timestamp-job.sh install`**: the automatic re-sign, a launchd job that runs every day at 09:00
  and after a missed run. It uses only the timestamp key and gets the R2/B2 keys from your keychain. A failure posts a
  notification.
- **Proven end to end on the box** with throwaway keys (deleted afterwards): the five real pinned models (1.7 GB, every
  file matched its pin) plus stub echo, nano and voice-service packages were published, verified, packed, then
  sideloaded and installed over a local server by the CLI's own code, followed by `--frozen --offline`, `engine`,
  `remove` and `prune`.
- **Model licenses checked.** All five may be mirrored, with notices. Those are in `THIRD_PARTY_NOTICES.md`:
  Parakeet TDT v2 is CC-BY-4.0, Nemotron streaming is the NVIDIA Open Model License, the Nemotron 3 diarizer is
  OpenMDW-1.1, and LocalVQE and Laya are Apache-2.0.

Not ready yet (ours, not yours): ctrl can't be published until the index accepts `extension.reads` (Bobs is fixing
it). No extension can pass the installer's notarization check until bare helpers get a stapleable check (ADR 0160).
The `inference.voice` XPC service doesn't exist yet. So the first publish is models only.

## Part A: the signed v1 build (needed for launch)

**A1. Apple Developer Program.** Join at <https://developer.apple.com/programs/enroll/> on your Apple ID (individual
or organization; organization needs a D-U-N-S number and takes longer). Then note your **Team ID**: developer.apple.com
→ **Account** → **Membership details**. **Send us:** the Team ID (10 characters, public).

**A2. Developer ID Application certificate.** Xcode → **Settings → Accounts** → your team → **Manage Certificates** →
**+** → **Developer ID Application**. Check it:

```sh
security find-identity -v -p codesigning   # shows "Developer ID Application: <Name> (<TEAMID>)"
```

**A3. Notarization credentials.** Make an app-specific password at <https://account.apple.com> → **Sign-In and
Security → App-Specific Passwords** (name it `qrtz-notary`), then save a notarytool profile in your keychain:

```sh
xcrun notarytool store-credentials qrtz-notary --apple-id "<your Apple ID email>" --team-id <TEAMID>
# paste the app-specific password when asked; check it with:
xcrun notarytool history --keychain-profile qrtz-notary
```

**A4. The first signed build, notarized and stapled.** From a clean checkout of `main` on your Mac:

```sh
bun install --frozen-lockfile && bun run check && bun run check:rust
export APPLE_SIGNING_IDENTITY="Developer ID Application: <Name> (<TEAMID>)"
unset APPLE_ID APPLE_PASSWORD APPLE_API_KEY   # tauri build signs only; sign-mac.sh notarizes after fixing the signatures
bun run make                                  # Swift helper + qrtz-snap + the qrtz command, then tauri build
APP=target/release/bundle/macos/qrtz.app      # under $CARGO_TARGET_DIR if you set one
scripts/release/sign-mac.sh "$APP"            # re-signs Contents/MacOS/*, reseals, checks, notarizes, staples, makes the .dmg
```

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

[hosting-setup.md](hosting-setup.md) has the details. This is the order, with what's already done marked.

**B1. Cloudflare zone.** Done: `qrtz.ai`'s nameservers moved to Cloudflare at about 4 PM PT on 2026-10-18. You only
check that the zone shows **Active** (dash.cloudflare.com → `qrtz.ai` → **Overview**) and that the website and mail
records came across ([§1](hosting-setup.md#1-cloudflare-account-and-the-qrtzai-zone), step 2).

**B2. R2 bucket `qrtz-modules`, the `modules.qrtz.ai` custom domain, cache rules, privacy settings.** Bobs does
this, on your go, in the Cloudflare session on his browser ([§2](hosting-setup.md#2-the-r2-bucket), steps 1–5). **You
do:** say go, then make the R2 API token yourself (R2 → **Manage API tokens** → **Create API token**, Object Read &
Write, only bucket `qrtz-modules`, §2 step 6). Keep it in your password manager, not in chat or Bobs's browser.

**B3. Backblaze B2 mirror.** You do [§3](hosting-setup.md#3-the-backblaze-b2-mirror): bucket `qrtz-modules-mirror`
(public), plus an application key scoped to it. **Send us** the bucket's S3 endpoint if it isn't
`s3.us-west-004.backblazeb2.com`. It's pinned in `module_hosts.json`.

**B4. Keys, offline (replaces hosting-setup §4's manual commands).** While online, run `cargo fetch` in the checkout
once. Then turn Wi-Fi off, plug in a USB stick (ideally encrypted APFS, named ROOTKEY-A) and run one of:

```sh
scripts/modules/release-keys.sh /Volumes/ROOTKEY-A keychain             # root-b in this Mac's login keychain
scripts/modules/release-keys.sh /Volumes/ROOTKEY-A /Volumes/ROOTKEY-B   # root-b on a second stick
```

Root 1 needs both root keys (threshold 2), so one lost or stolen key can't change who is trusted. It prints
`root1Sha256 = <64 hex>`. **Send us** that line (it's public). Then eject the stick(s) and keep root-a and root-b in
different places. Never send any `*.key` file.

**B5. Pin the root.** Ours: once you send the hash, we commit it as `root1Sha256` in
`crates/qrtz-app/src/module_hosts.json` (checked by `offline_graph`). Nothing for you to do, but a build made before
that commit can't install packages, so cut a new signed build afterwards (A4).

**B6. The first publish (models only).** On the release Mac: `brew install rclone`, open qrtz once so it downloads its
models (onboarding, and ctrl's model in its settings), then:

```sh
export QRTZ_TEAM_ID=<TEAMID>
export R2_ACCOUNT_ID=<Cloudflare account id: R2 overview page, right column>
export R2_ACCESS_KEY_ID=<from step B2> R2_SECRET_ACCESS_KEY="$(security find-generic-password -s qrtz-r2 -w)"
export B2_KEY_ID=<from step B3> B2_APPLICATION_KEY="$(security find-generic-password -s qrtz-b2 -w)"
scripts/modules/publish.sh --no-upload    # build + verify only: check the list it prints
scripts/modules/publish.sh                # upload to R2, then B2, then check both hosts
```

(Store the two secrets first: `security add-generic-password -s qrtz-r2 -a <R2 access key id> -w`, and the same
with `-s qrtz-b2` and the B2 key id.) **Send us** the last 10 lines of the output. **Then turn on the automatic
re-sign** (a timestamp older than 8 days stops new installs everywhere):

```sh
echo "$R2_ACCOUNT_ID" > ~/.qrtz-keys/r2-account-id
scripts/modules/timestamp-job.sh install    # daily at 09:00, and at login/wake after a missed run
scripts/modules/timestamp-job.sh status     # last run and result; log: ~/Library/Logs/qrtz-timestamp.log
```

**B7. Back up** `~/qrtz-publish-out` (the record of everything published) and `~/.qrtz-keys` to an encrypted disk.
