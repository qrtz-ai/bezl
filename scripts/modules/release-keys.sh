#!/usr/bin/env bash
# One-time: make the package index's four Ed25519 keys and root 1, then print root1Sha256 to pin (ADR 0153 §3,
# docs/modules/production-setup.md B4). Run it on Jared's Mac with Wi-Fi OFF, never on a shared or CI machine.
#
#   scripts/modules/release-keys.sh /Volumes/ROOTKEY-A keychain          root-b in this Mac's login keychain
#   scripts/modules/release-keys.sh /Volumes/ROOTKEY-A /Volumes/ROOTKEY-B  root-b on a second stick
#
# Root 1 has threshold 2: it is signed by both root keys, and a later root (a key rotation) needs both again, so one
# lost or stolen root key can't rotate the trust. Keep the two apart.
#
# What it writes (keygen never overwrites; a second run refuses instead of replacing a key):
#   <root-a>/root-a.key + .pub     offline root key 1 (a USB stick, kept unplugged)
#   root-b                         offline root key 2: the login keychain (service "qrtz-root-b", not iCloud-synced)
#                                  or <root-b>/root-b.key + .pub on a second stick; its public half also goes to
#                                  ~/.qrtz-keys/root-b.pub
#   ~/.qrtz-keys/index.key + .pub, timestamp.key + .pub   the release machine's signing keys (dir 0700, keys 0600)
#   ~/qrtz-publish-out/v1/root/1.root.json + .sig         root 1 (threshold 2), signed by root-a and root-b
#   ~/qrtz-publish-out/ROOT1_SHA256                       the pin (public)
# Only the public root sha256 is printed. No private key is printed, copied to the repo or left in a temp dir.
#
# Environment (optional): QRTZ_KEYS (default ~/.qrtz-keys), QRTZ_PUBLISH_OUT (default ~/qrtz-publish-out),
# QRTZ_PUBLISH_BIN (a built qrtz-publish; default: cargo build --release --offline -p qrtz-publish).
# QRTZ_KEYS_ALLOW_ONLINE=1 skips the offline check (only for the throwaway keys of a test run).
set -euo pipefail

die() {
  echo "release-keys: $*" >&2
  exit 1
}
[[ $# -eq 2 ]] || die "usage: $0 <root-a dir (USB stick)> <root-b dir (second stick) | keychain>"
root_a="$1"
root_b="$2"
keys="${QRTZ_KEYS:-$HOME/.qrtz-keys}"
out="${QRTZ_PUBLISH_OUT:-$HOME/qrtz-publish-out}"
repo="$(cd "$(dirname "$0")/../.." && pwd)"

[[ -d "$root_a" ]] || die "$root_a: not mounted (plug in the stick)"
if [[ "$root_b" == keychain ]]; then
  command -v security >/dev/null || die "keychain: needs macOS's security tool"
  security find-generic-password -s qrtz-root-b -a qrtz >/dev/null 2>&1 && die "the keychain already has qrtz-root-b"
else
  [[ -d "$root_b" ]] || die "$root_b: not mounted (plug in the second stick)"
  [[ "$(cd "$root_a" && pwd -P)" != "$(cd "$root_b" && pwd -P)" ]] || die "root-a and root-b must be in different places"
fi
[[ -e "$out/v1/root/1.root.json" ]] && die "$out already has root 1; this is a one-time step (rotation: qrtz-publish root --prev-keys)"

# Offline: no default route (macOS: route; Linux: ip).
if [[ "${QRTZ_KEYS_ALLOW_ONLINE:-}" != "1" ]]; then
  if route -n get default >/dev/null 2>&1 || ip route show default 2>/dev/null | grep -q default; then
    die "this machine has a network route; turn Wi-Fi off (and unplug Ethernet), then run again"
  fi
fi

bin="${QRTZ_PUBLISH_BIN:-}"
if [[ -z "$bin" ]]; then
  # Build from the already-fetched crates; run `cargo fetch` once while online if this fails.
  (cd "$repo" && cargo build --release --offline -p qrtz-publish) ||
    die "build failed; while online run: cargo fetch && cargo build --release -p qrtz-publish"
  bin="${CARGO_TARGET_DIR:-$repo/target}/release/qrtz-publish"
fi
[[ -x "$bin" ]] || die "$bin: not found"

umask 077
mkdir -p "$keys"
chmod 700 "$keys"
# Root 1 is signed by both root keys (threshold 2): a scratch dir holds the two root private keys and the public
# keys only for this one command, then is wiped. Nothing extra is written to either root location.
scratch="$(mktemp -d)"
wipe() { # overwrite before unlinking (rm -P on macOS; shred elsewhere when present)
  [[ -f "$1" ]] || return 0
  rm -P "$1" 2>/dev/null || shred -u "$1" 2>/dev/null || rm -f "$1"
}
cleanup() {
  wipe "$scratch/root-a.key"
  wipe "$scratch/root-b.key"
  rm -rf "$scratch"
}
trap cleanup EXIT

"$bin" keygen --dir "$root_a" --role root-a
if [[ "$root_b" == keychain ]]; then
  "$bin" keygen --dir "$scratch" --role root-b
  # Login keychain, this Mac only (security's generic passwords aren't iCloud-synchronized).
  security add-generic-password -s qrtz-root-b -a qrtz -l "qrtz package index root key B (offline)" \
    -w "$(cat "$scratch/root-b.key")"
  echo "root-b: stored in the login keychain (service qrtz-root-b)"
else
  "$bin" keygen --dir "$root_b" --role root-b
  install -m 600 "$root_b/root-b.key" "$scratch/root-b.key"
  install -m 644 "$root_b/root-b.pub" "$scratch/root-b.pub"
fi
"$bin" keygen --dir "$keys" --role index
"$bin" keygen --dir "$keys" --role timestamp
install -m 600 "$root_a/root-a.key" "$scratch/root-a.key"
install -m 644 "$root_a/root-a.pub" "$keys/index.pub" "$keys/timestamp.pub" "$scratch/"
install -m 644 "$scratch/root-b.pub" "$keys/root-b.pub"
install -m 644 "$root_a/root-a.pub" "$keys/root-a.pub"
mkdir -p "$out"
line="$("$bin" root --keys "$scratch" --out "$out" --threshold 2)"
sha="$(printf '%s\n' "$line" | sed -n 's/^root 1: sha256 \([0-9a-f]\{64\}\)$/\1/p')"
[[ -n "$sha" ]] || die "unexpected output from qrtz-publish root: $line"
printf '%s\n' "$sha" >"$out/ROOT1_SHA256"
chmod 644 "$out/ROOT1_SHA256"

cat <<MSG

Done. Root 1 is in $out/v1/root/.

  root1Sha256 = $sha

Send that line to Grok Bot (it's public). It becomes "root1Sha256" in crates/qrtz-app/src/module_hosts.json.

Root 1 needs both root keys (threshold 2). Now: eject the stick(s) and keep root-a and root-b in different places. Keep $keys on this Mac only (back it up to an
encrypted disk, never to iCloud Drive, Dropbox, a repo or chat). Wi-Fi can go back on.
MSG
