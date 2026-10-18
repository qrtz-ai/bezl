# Module hosting: what Jared sets up

The code side is done ([ADR 0153](../adr/0153-module-hosting-the-signed-index-and-sideloads.md)): the signed index,
the installer, the `.qrtzpkg` sideload and the `qrtz-publish` tool. Nothing is published and no key exists yet.
These steps need your accounts and your hands. They take about an hour; nothing here costs money until real
traffic (R2: 10 GB storage and 10M reads a month free, no egress fees; B2: 10 GB free).

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
5. Privacy: zone → **Analytics & Logs**: don't enable Logpush or Web Analytics for `modules.qrtz.ai`. Don't add
   Workers, Page Rules that set cookies, or Bot Fight Mode on that hostname (the app sends no cookies).
6. **R2 → Manage API tokens → Create API token**: permission **Object Read & Write**, only bucket `qrtz-modules`.
   Keep the access key id and secret in your password manager; they go into `rclone` on the release machine only.

## 3. The Backblaze B2 mirror

1. Create an account at <https://www.backblaze.com/sign-up/cloud-storage> (two-factor on).
2. **Buckets → Create a bucket**: name `qrtz-modules-mirror` (if taken, pick another and tell me), files
   **Public**, encryption off, object lock off.
3. Note the bucket's **Endpoint** (e.g. `s3.us-west-004.backblazeb2.com`). The mirror URL is
   `https://<bucket>.<endpoint>`, e.g. `https://qrtz-modules-mirror.s3.us-west-004.backblazeb2.com`. If the name or
   region differ from that, send me the URL; it's pinned in `crates/qrtz-app/src/module_hosts.json` and checked by
   `offline_graph`.
4. **Application Keys → Add a new key**: only bucket `qrtz-modules-mirror`, Read and Write. Password manager.

## 4. Keys (offline)

Four Ed25519 keys. The two root keys never touch a networked machine after this step.

| Key                | Lives                                           | Used for                                    |
| ------------------ | ----------------------------------------------- | ------------------------------------------- |
| `root-a`, `root-b` | two encrypted USB sticks in two places, offline | signing a new root (rare: rotation, yearly) |
| `index`            | the release machine (your Mac), `~/.qrtz-keys/` | every publish                               |
| `timestamp`        | the release machine (later a scheduled job)     | the weekly re-sign                          |

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
`module_hosts.json`. Until it's there, the app refuses every network install. Eject both sticks and store them apart.
Each key file is 0600; `qrtz-publish` refuses one that isn't. Instead of `--keys`, the env vars
`QRTZ_PUBLISH_INDEX_KEY` / `QRTZ_PUBLISH_TIMESTAMP_KEY` (and `QRTZ_PUBLISH_ROOT_KEYS`) work, e.g. read from the
macOS Keychain in a shell; never put them in a file in the repo.

## 5. rclone on the release machine

```sh
brew install rclone
rclone config   # add "r2": type s3, provider Cloudflare, the R2 token, endpoint https://<account-id>.r2.cloudflarestorage.com
                # add "b2": type s3, provider Other (or b2), the B2 key, endpoint https://s3.<region>.backblazeb2.com
```

## 6. Each publish

```sh
P=target/release/qrtz-publish
$P build --out ~/qrtz-publish-out --spec publish.json --keys ~/.qrtz-keys
$P verify --out ~/qrtz-publish-out --root-sha256 <root 1 sha256>
# immutable objects first, the timestamp last, never --delete:
rclone copy ~/qrtz-publish-out r2:qrtz-modules --exclude v1/timestamp.json --exclude v1/timestamp.json.sig --immutable
rclone copy ~/qrtz-publish-out r2:qrtz-modules --include 'v1/timestamp.json*'
rclone copy ~/qrtz-publish-out b2:qrtz-modules-mirror --exclude v1/timestamp.json --exclude v1/timestamp.json.sig --immutable
rclone copy ~/qrtz-publish-out b2:qrtz-modules-mirror --include 'v1/timestamp.json*'
```

`publish.json` lists the built bundles: `{"teamId": "<Developer ID team>", "packages": [{"id": "echo", "version":
"0.3.0", "dir": "build/echo"}, {"id": "model:parakeet-tdt-v2", "version": "1.0.0", "dir": "models/parakeet"}],
"revoked": []}`. A module dir holds its `module.json` and its signed, notarized, **stapled** `.app`. Keep
`~/qrtz-publish-out` (or back it up): it's the record of everything published.

## 7. Weekly

The timestamp expires 7 days after it's signed; after 8 days every installed qrtz refuses new installs and updates
(modules already installed keep working). Re-sign at least weekly:

```sh
$P timestamp --out ~/qrtz-publish-out --keys ~/.qrtz-keys && rclone copy ~/qrtz-publish-out r2:qrtz-modules --include 'v1/timestamp.json*' && rclone copy ~/qrtz-publish-out b2:qrtz-modules-mirror --include 'v1/timestamp.json*'
```

A launchd job on the release Mac can do it daily; or later in CI once GitHub billing is fixed (then the timestamp key
moves to a CI secret, and only that key).

## 8. Pulling a bad release

Never delete it. Add `{"package": "echo", "versions": ["0.3.0"], "reason": "…"}` to `revoked` in `publish.json`,
run step 6 again. Installed copies are disabled on their next check; new installs refuse it.

## 9. Offline file for an air-gapped Mac

```sh
$P pack --out ~/qrtz-publish-out --module echo --version 0.3.0 --to echo-0.3.0.qrtzpkg
```

The file carries the roots, the signed index and every blob echo needs; settings → modules → install from file
asks once and shows how old its revocation info is.

## Still open (not blocking the setup)

- Developer ID signing, notarization and stapling of the module apps (docs/release.md); until then manifests use
  the placeholder team and the installer refuses them.
- The LocalVQE and Laya licenses must allow mirroring before those models are published.
