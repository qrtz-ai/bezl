# Production setup: what's done, and Jared's checklist

v1 ships one signed, notarized qrtz.app and its package hosting: `modules.qrtz.ai`, the signed index and
`qrtz install …` over the network. echo, nano and ctrl are built in (the bundled `qrtz-helper`), and so is snap
(`qrtz-snap`, ADR 0159). Part A is the signed build. Part B is the hosting.

## What's done (box side, 2026-10-18)

- **The `qrtz` command is in every build** as `qrtz.app/Contents/MacOS/qrtz`, an `externalBin` that
  `bun run build:helper` builds. The app's own executable is now `Contents/MacOS/qrtz-app`. The bundle, its identifier
  and its Privacy entries are unchanged, and the login item is fixed on the first launch
  ([ADR 0162](../adr/0162-the-qrtz-command-ships-as-contents-macos-qrtz.md)). `qrtz link` puts `~/.local/bin/qrtz`
  on your PATH as a symlink into the bundle. Until `root1Sha256` is pinned it refuses every install, as designed.
- **Every executable in `Contents/MacOS` gets the right signature.** `tauri build` gives every bundled executable the
  app's entitlements, so `scripts/release/sign-mac.sh` signs each one again: `qrtz` with none and `qrtz-snap` as
  `ai.qrtz.snap` with none. Then it reseals and checks the app, notarizes and staples it, and makes, notarizes and
  staples the `.dmg` (A4). The list it signs is checked against `docs/modules/confinement-map.json` by a test.
- **Package manifests**: `modules/<name>/qrtz.json` for echo, nano, ctrl and snap, and
  `tools/qrtz-publish/packages/inference.voice/qrtz.json`.
- **`scripts/modules/release-keys.sh`**: the whole key ceremony in one command (offline only). Root 1 has threshold 2,
  so it is signed by both root keys and a key rotation needs both.
- **`scripts/modules/publish.sh`**: build, verify, upload to R2, and a live check, with credentials from env
  vars only. `--no-upload` builds and verifies; `--timestamp` is the re-sign. Sole host: `modules.qrtz.ai` (ADR 0167).
- **`scripts/modules/timestamp-job.sh install`**: the automatic re-sign, a launchd job that runs every day at 09:00
  and after a missed run. It uses only the timestamp key and gets the R2 key from your keychain. A failure posts a
  notification.
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

Not ready yet (ours, not yours): no extension can pass the installer's notarization check until bare helpers get a stapleable check (ADR 0162).
The `inference.voice` XPC service doesn't exist yet. So the first publish is models only. echo, nano, ctrl and
snap are built into the app for v1, so nothing waits on that.

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

[hosting-setup.md](hosting-setup.md) has the details. This is the order, with what's already done marked.

**B1. Cloudflare zone.** Done: `qrtz.ai`'s nameservers moved to Cloudflare at about 4 PM PT on 2026-10-18. You only
check that the zone shows **Active** (dash.cloudflare.com → `qrtz.ai` → **Overview**) and that the website and mail
records came across ([§1](hosting-setup.md#1-cloudflare-account-and-the-qrtzai-zone), step 2).

**B2. R2 bucket `qrtz-modules`, the `modules.qrtz.ai` custom domain, cache rules, privacy settings.** Bobs does
this, on your go, in the Cloudflare session on his browser ([§2](hosting-setup.md#2-the-r2-bucket), steps 1–5). **You
do:** say go, then make the R2 API token yourself (R2 → **Manage API tokens** → **Create API token**, Object Read &
Write, only bucket `qrtz-modules`, §2 step 6). Keep it in your password manager, not in chat or Bobs's browser.

**B3. No package-host mirror.** Done by decision (2026-10-18, ADR 0167): Jared chose not to run Backblaze B2.
`modules.qrtz.ai` is the sole package host; `module_hosts.json` has `mirrors: []`. Skip the old B2 setup steps.

**B4. Keys, offline (replaces hosting-setup §4's manual commands).** While online, run `cargo fetch` in the checkout
once. Then turn Wi-Fi off, plug in a USB stick (ideally encrypted APFS, named ROOTKEY-A) and run one of:

```sh
scripts/modules/release-keys.sh /Volumes/ROOTKEY-A keychain             # root-b in this Mac's login keychain
scripts/modules/release-keys.sh /Volumes/ROOTKEY-A /Volumes/ROOTKEY-B   # root-b on a second stick
```

Root 1 needs both root keys (threshold 2), so one lost or stolen key can't change who is trusted. It prints
`root1Sha256 = <64 hex>`. **Send us** that line (it's public). Then eject the stick(s) and keep root-a and root-b in
different places. Never send any `*.key` file.

**Done 2026-10-18** (Jared's Mac, run with `QRTZ_KEYS_ALLOW_ONLINE=1`, so the offline check was skipped):
`root1Sha256 = 02dbb15f7927e343c5747a4d085a564775a49aecec2f854c7d028c4935afaa7d`, root 1 expires 2027-10-18.

**B5. Pin the root. Done 2026-10-18.** `root1Sha256` is set in `crates/qrtz-app/src/module_hosts.json` (checked by
`offline_graph`). The public `1.root.json` and `.sig` are checked in at `crates/qrtz-modules/roots/`, and
`crates/qrtz-modules/tests/pinned_root.rs` checks that they hash to the pin and carry threshold 2 met by both root
keys. They are also live at `https://modules.qrtz.ai/v1/root/1.root.json` (+ `.sig`). A build made before that
commit can't install packages, so cut a new signed build afterwards (A4).

**B6. The first publish (models only).** On the release Mac: `brew install rclone`, open qrtz once so it downloads its
models (onboarding, and ctrl's model in its settings), then:

```sh
# QRTZ_TEAM_ID defaults to MR26CCZ496
export R2_ACCOUNT_ID=<Cloudflare account id: R2 overview page, right column>
export R2_ACCESS_KEY_ID=<from step B2> R2_SECRET_ACCESS_KEY="$(security find-generic-password -s qrtz-r2 -w)"
scripts/modules/publish.sh --no-upload    # build + verify only: check the list it prints
scripts/modules/publish.sh                # upload to R2, then check modules.qrtz.ai
```

(Store the R2 secret first: `security add-generic-password -s qrtz-r2 -a <R2 access key id> -w`.) **Send us** the
last 10 lines of the output. **Then turn on the automatic re-sign** (a timestamp older than 8 days stops new
installs everywhere):

```sh
echo "$R2_ACCOUNT_ID" > ~/.qrtz-keys/r2-account-id
scripts/modules/timestamp-job.sh install    # daily at 09:00, and at login/wake after a missed run
scripts/modules/timestamp-job.sh status     # last run and result; log: ~/Library/Logs/qrtz-timestamp.log
```

**B7. Back up** `~/qrtz-publish-out` (the record of everything published) and `~/.qrtz-keys` to an encrypted disk.
