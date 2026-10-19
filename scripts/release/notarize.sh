#!/usr/bin/env bash
# Submit one file (the app's zip or the .dmg) to Apple's notary service, wait, and fail unless Apple accepted it.
# Called by sign-mac.sh (which sign-notarize.sh wraps).
#
# Why: `xcrun notarytool submit --wait` can exit 0 when Apple rejects the upload (status Invalid), and the script
# would go on to staple a ticket that doesn't exist. So the result comes back as JSON (--output-format json, a flat
# object with id, status and message) and only status "Accepted" passes. On anything else this prints the
# submission id and the `notarytool log` command that shows Apple's reasons, then exits 1.
#
# Usage: scripts/release/notarize.sh <file> <keychain-profile>
set -uo pipefail

file="${1:?usage: notarize.sh <file> <keychain-profile>}"
profile="${2:?usage: notarize.sh <file> <keychain-profile>}"
err="$(mktemp)"
trap 'rm -f "$err"' EXIT

out="$(xcrun notarytool submit "$file" --keychain-profile "$profile" --wait --output-format json 2>"$err")"
rc=$?

# The last value of a top-level string field ("id", "status") in notarytool's JSON. No jq or python needed.
field() { sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" <<<"$out" | tail -n 1; }
id="$(field id)"
status="$(field status)"

if [[ "$status" == "Accepted" ]]; then
  echo "  notarized: $(basename "$file") (submission $id, Accepted)"
  exit 0
fi

{
  echo "notarization failed for $file: status ${status:-unknown}, notarytool exit $rc"
  [[ -n "$(field message)" ]] && echo "  apple: $(field message)"
  if [[ -n "$id" ]]; then
    echo "  submission id: $id"
    echo "  why: xcrun notarytool log $id --keychain-profile $profile"
  else
    echo "  no submission id in notarytool's output (was the upload refused before it started?)"
  fi
  [[ -n "$out" ]] && echo "  notarytool output: $out"
  [[ -s "$err" ]] && sed 's/^/  notarytool: /' "$err"
} >&2
exit 1
