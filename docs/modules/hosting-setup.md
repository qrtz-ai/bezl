# Module hosting: what Jared sets up

The code side is done ([ADR 0153](../adr/0153-module-hosting-the-signed-index-and-sideloads.md), and the package
manager on top of it, [ADR 0157](../adr/0157-qrtz-is-its-own-package-manager.md)): the signed index, the installer,
the `qrtz` command, the `.qrtzpkg` sideload and the `qrtz-publish` tool. Nothing is published and no key exists yet.
These steps need your accounts and your hands. They take about an hour; nothing here costs money until real
traffic (R2: 10 GB storage and 10M reads a month free, no egress fees).

Never paste a private key (`*.key`) into chat, a ticket, the repo, CI or a shared drive. Public keys (`*.pub`) and
the root sha256 are fine to share.

## 1. Cloudflare account and the qrtz.ai zone

1. Create an account at <https://dash.cloudflare.com/sign-up> (turn on two-factor auth).
2. **Add a site** → `qrtz.ai` → Free plan. Cloudflare scans the existing DNS records; check every record it found
   against your current DNS (website, mail MX/TXT/SPF/DKIM) before continuing.
3. At your registrar, replace the nameservers with the two Cloudflare shows. Wait until the zone says **Active**
   (minutes to a few hours).

## 2. The R2 bucket

1. **R2 Object Storage** → enable R2 (it asks for a card; the free tier covers this).
2. **Create bucket**: name `qrtz-modules`, location automatic (or Eastern North America), storage class Standard.
3. Bucket → **Settings**:
   - **Public access → Custom domains → Connect domain**: `modules.qrtz.ai`. Cloudflare adds the DNS record and the
     certificate itself. Leave the `r2.dev` URL **disabled**.
   - Leave CORS empty (the app isn't a browser).
4. Zone `qrtz.ai` → **Caching → Cache Rules** → create a rule for hostname `modules.qrtz.ai`:
   - URI path starts with `/v1/timestamp.json`: eligible for cache, edge TTL 5 minutes.
   - everything else under `/v1/`: eligible for cache, edge TTL 1 year (those objects never change).
   - Files are whole objects now (no chunks, ADR 0157 §5). Cloudflare doesn't edge-cache an object above 512 MB (the
     Nemotron encoder is 589 MB); it's still served from R2, in byte ranges, so nothing to configure.
5. Privacy: zone → **Analytics & Logs**: don't enable Logpush or Web Analytics for `modules.qrtz.ai`. Don't add
   Workers, Page Rules that set cookies, or Bot Fight Mode on that hostname (the app sends no cookies).
6. **R2 → Manage API tokens → Create API token**: permission **Object Read & Write**, only bucket `qrtz-modules`.
   Keep the access key id and secret in your password manager; they go into `rclone` on the release machine only.

## 3. No package-host mirror (ADR 0167)

Jared decided (2026-10-18) not to run a Backblaze B2 mirror. `modules.qrtz.ai` (Cloudflare R2, bucket
`qrtz-modules`) is the sole package host. `module_hosts.json` pins only that primary (`mirrors: []`). Model files
are never on it (ADR 0171): the signed tree gives each one a Hugging Face URL at a pinned commit, the client fetches
it only from there and checks it against the signed sha256 and size. Proposal §4's B2 cold-copy plan is withdrawn; see [ADR 0167](../adr/0167-no-backblaze-b2-package-host-mirror.md).

## 4. Keys (offline)

`scripts/modules/release-keys.sh /Volumes/ROOTKEY-A /Volumes/ROOTKEY-B` does all of this section in one command
(docs/modules/production-setup.md); the steps below are what it runs. `scripts/modules/publish.sh` does §5–§7
with credentials from env vars.

Four Ed25519 keys. The two root keys never touch a networked machine after this step.

| Key                | Lives                                           | Used for                                    |
| ------------------ | ----------------------------------------------- | ------------------------------------------- |
| `root-a`, `root-b` | two encrypted USB sticks in two places, offline | signing a new root (rare: rotation, yearly) |
| `index`            | the release machine (your Mac), `~/.qrtz-keys/` | every publish                               |
| `timestamp`        | the release machine (timestamp-job.sh, daily)   | the re-sign                                 |

On an offline machine (or your Mac with Wi-Fi off), from a checkout:

```sh
cargo build --release -p qrtz-publish
P=target/release/qrtz-publish
$P keygen --dir /Volumes/ROOTKEY-A --role root-a
$P keygen --dir /Volumes/ROOTKEY-B --role root-b
mkdir -m 700 ~/.qrtz-keys
$P keygen --dir ~/.qrtz-keys --role index
$P keygen --dir ~/.qrtz-keys --role timestamp
# root 1 needs root-a's private key plus root-b's public half:
cp /Volumes/ROOTKEY-B/root-b.pub /Volumes/ROOTKEY-A/
cp ~/.qrtz-keys/index.pub ~/.qrtz-keys/timestamp.pub /Volumes/ROOTKEY-A/
$P root --keys /Volumes/ROOTKEY-A --out ~/qrtz-publish-out
```

`root` prints `root 1: sha256 <hex>`. Send me that hex (it's public): it becomes `root1Sha256` in
`module_hosts.json`. Until it's there, the app refuses every network install, and ctrl's Laya model too: ctrl
reads it in its own process, so it needs a fresh signed revocation list like code (ADR 0157 §6, §9; ctrl falls back
to its heuristic scorer). The helper's speech models don't wait for it. Eject both sticks and store them apart.
Each key file is 0600; `qrtz-publish` refuses one that isn't. Instead of `--keys`, the env vars
`QRTZ_PUBLISH_INDEX_KEY` / `QRTZ_PUBLISH_TIMESTAMP_KEY` (and `QRTZ_PUBLISH_ROOT_KEYS`) work, e.g. read from the
macOS Keychain in a shell; never put them in a file in the repo.

## 5. rclone on the release machine

```sh
brew install rclone
rclone config   # add "r2": type s3, provider Cloudflare, the R2 token, endpoint https://<account-id>.r2.cloudflarestorage.com
```

## 6. Each publish

```sh
P=target/release/qrtz-publish
$P build --out ~/qrtz-publish-out --spec publish.json --keys ~/.qrtz-keys
$P verify --out ~/qrtz-publish-out --root-sha256 <root 1 sha256>
# immutable objects first, the timestamp last, never --delete:
rclone copy ~/qrtz-publish-out r2:qrtz-modules --exclude v1/timestamp.json --exclude v1/timestamp.json.sig --immutable
rclone copy ~/qrtz-publish-out r2:qrtz-modules --include 'v1/timestamp.json*'
```

`publish.json` (paths relative to it):

```json
{
  "teamId": "<Developer ID team>",
  "packages": [{ "dir": "build/nano" }],
  "pinnedModels": [
    "crates/qrtz-app/src/local_models.json",
    "crates/qrtz-app/src/ctrl_laya_model.json"
  ],
  "models": [
    { "name": "model.parakeet-tdt-v2-en", "dir": "models/parakeet-tdt-v2" },
    { "name": "model.nemotron", "dir": "models/nemotron-560ms" }
  ],
  "revoked": []
}
```

- Each `packages` dir holds the package's `qrtz.json` (name, version, kind, dependency ranges; no `scripts`, they're
  refused) and its files: an extension's signed, notarized, **stapled** helper, a service's `.xpc`.
- A model is published from the files you already downloaded from Hugging Face, e.g. the app's own
  `~/Library/Application Support/ai.qrtz.desktop/models/parakeet-tdt-v2` after the speech download (settings → speech, or switching echo on). Every file must match its
  pin in the `pinnedModels` file, and the published package is exactly the tree the app pins (same sha256). The
  files are only hashed (ADR 0171): the output dir gets the model's `qrtz.json` and tree, the app downloads the files
  from their pinned Hugging Face URLs, and qrtz-publish keeps a copy in `<out>.model-blobs/` for offline packs.
- Keep `~/qrtz-publish-out` (or back it up): it's the record of everything published.

## 7. Weekly

The timestamp expires 7 days after it's signed; after 8 days every installed qrtz refuses new installs and updates
(modules already installed keep working). Re-sign at least weekly:

```sh
$P timestamp --out ~/qrtz-publish-out --keys ~/.qrtz-keys && rclone copy ~/qrtz-publish-out r2:qrtz-modules --include 'v1/timestamp.json*'
```

`scripts/modules/timestamp-job.sh install` does this automatically: a launchd job on the release Mac, daily at 09:00
and after a missed run, with the R2 key from the login keychain and only the timestamp key. Later it can move to
CI once GitHub billing is fixed (then the timestamp key moves to a CI secret, and only that key).

## 8. Pulling a bad release

Never delete it. Add `{"package": "echo", "versions": ["0.3.0"], "reason": "…"}` to `revoked` in `publish.json`,
run step 6 again. Installed copies are disabled on their next check; new installs refuse it.

## 9. Offline file for an air-gapped Mac

```sh
$P pack --out ~/qrtz-publish-out --name echo --version 0.3.0 --to echo-0.3.0.qrtzpkg
```

The file carries the roots, the signed index and nano with everything it depends on (its `model.*` packages; ADR
0174: echo is bundled, `inference.voice` cut); on the other Mac, settings → install from file, or `qrtz install ./nano-0.3.0.qrtzpkg`,
asks once and shows how old its revocation info is.

## 10. The `qrtz` command (optional, on your Mac)

`qrtz install echo`, `qrtz remove echo`, `qrtz update`, `qrtz prune`, `qrtz ls`. It's inside the app
(`/Applications/qrtz.app/Contents/MacOS/qrtz`); to have it on `PATH`, run that once with `link` (it makes
`~/.local/bin/qrtz`, a symlink into the app; no `sudo`, nothing in `/usr/local`). Until `root1Sha256` is pinned it
refuses network installs, like the app.

## Still open (not blocking the setup)

- Developer ID signing, notarization and stapling of the module apps (docs/release.md); until then manifests use
  the placeholder team and the installer refuses them.
- Done: every published model may be mirrored (Parakeet TDT v2 CC-BY-4.0, Nemotron streaming NVIDIA Open Model
  License, Nemotron 3 diarizer OpenMDW-1.1, LocalVQE and Laya Apache-2.0); attribution is in THIRD_PARTY_NOTICES.md.
  Laya's download still needs the live, fresh index above.
- Done (ADR 0162): `qrtz` ships as the externalBin `qrtz.app/Contents/MacOS/qrtz`; the app's own executable is
  `Contents/MacOS/qrtz-app`.
