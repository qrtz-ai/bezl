#!/usr/bin/env bash
# The release gate (0.2.4 post-mortem): a qrtz.app is shippable or installable only if this passes. Shared by
# sign-notarize.sh (release:mac, after notarize and staple), scripts/app/publish-local.sh (before upload) and
# .github/workflows/app-release.yml, so CI and local check the same thing.
#
# Checks, each failing nonzero with a message:
#   - every executable in Contents/MacOS satisfies the requirement a release build launches it under
#     (crates/qrtz-app/src/installed.rs launch_requirement): qrtz-app ai.qrtz.desktop, each bundled helper the
#     code.bundleId and code.designatedRequirement of its modules/*/module.json (QRT-255; qrtz-echo ai.qrtz.echo),
#     qrtz ai.qrtz.cli, Developer ID, team MR26CCZ496; an unknown executable fails;
#   - each executable's entitlements are exactly its allowlist (ents_for below): qrtz-app apple-events, allow-jit,
#     audio-input, calendars; qrtz-echo audio-input; qrtz none. get-task-allow anywhere fails;
#   - every Mach-O anywhere in the bundle (Frameworks, Helpers, XPCServices, Library/LoginItems, Resources, ...; found
#     by magic bytes) and every nested .app/.framework/.xpc/.appex/.bundle/.dylib is one of the three known
#     executables, with the signing identifier allowed for its path, else it fails;
#   - no archive (gzip, zip, xz, bzip2, zstd, tar, 7z, xar, cpio, ...; by magic bytes) unless allowlisted, and no file
#     holding the qrtz-dash-net marker (ADR 0231, M5);
#   - no symlink in Contents resolves outside the bundle (or dangles);
#   - codesign --verify --deep --strict on the app (designated requirement ai.qrtz.desktop, notarized);
#   - spctl -a -vv says Notarized Developer ID, and xcrun stapler validate passes.
# The 0.2.4 candidate failed the first two: tauri build's signatures (file-name identifiers, the app's entitlements
# on every binary) had not been replaced by sign-mac.sh, so the app refused to launch echo.
#
# Usage: scripts/release/verify-bundle.sh <qrtz.app>   (QRTZ_TEAM_ID overrides MR26CCZ496)
set -uo pipefail

app="${1:?usage: verify-bundle.sh <qrtz.app>}"
team="${QRTZ_TEAM_ID:-MR26CCZ496}"
macos="$app/Contents/MacOS"
fails=0
fail() { echo "verify-bundle: FAIL: $*" >&2; fails=$((fails + 1)); }
ok() { echo "verify-bundle: ok: $*"; }

[[ -d "$macos" ]] || { echo "verify-bundle: FAIL: $app is not an app bundle (no Contents/MacOS)" >&2; exit 1; }

# QRT-255: the bundled module helpers (qrtz-echo today: ai.qrtz.echo, audio-input) come from each modules/*/module.json
# (bundled-helpers.sh), the file the launcher's requirement comes from, so this gate and sign-mac.sh can't drift from
# it. Unreadable or inconsistent module.json fails the gate.
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=bundled-helpers.sh
source "$repo/scripts/release/bundled-helpers.sh"
helpers="$(bundled_helpers "$repo/modules")" || { echo "verify-bundle: FAIL: can't read the bundled helpers from $repo/modules/*/module.json" >&2; exit 1; }
helper_field() { awk -F'\t' -v n="$1" -v c="$2" '$1 == n { print $c; exit }' <<<"$helpers"; }
helper_names() { cut -f1 <<<"$helpers"; }
ident_for() {
  case "$1" in qrtz-app) echo ai.qrtz.desktop ;; qrtz) echo ai.qrtz.cli ;; *) helper_field "$1" 2 ;; esac
}
ents_for() {
  local f
  case "$1" in
    qrtz-app) echo "com.apple.security.automation.apple-events com.apple.security.cs.allow-jit com.apple.security.device.audio-input com.apple.security.personal-information.calendars" ;;
    qrtz) echo "" ;;
    *)
      f="$(helper_field "$1" 3)"
      [[ -n "$f" && "$f" != none ]] && grep -o '<key>[^<]*</key>' "$repo/$f" | sed 's/<[^>]*>//g' | sort | tr '\n' ' ' | sed 's/ $//'
      ;;
  esac
}
ent_keys() { codesign -d --entitlements - --xml "$1" 2>/dev/null | plutil -convert xml1 -o - - 2>/dev/null | grep -o '<key>[^<]*</key>' | sed 's/<[^>]*>//g' | sort | tr '\n' ' ' | sed 's/ $//'; }

for name in qrtz-app qrtz $(helper_names); do [[ -x "$macos/$name" ]] || fail "$name is missing from Contents/MacOS"; done

# QRT-275: the bundle carries its own notices and license texts (Contents/Resources). Each must be there, non-empty,
# and start with its own license's title. scripts/check-notices.mjs keeps this list, tauri.conf.json and the repo
# files in step.
required_notices=(
  "THIRD_PARTY_NOTICES.md|# Third-party notices"
  "licenses/AGPL-3.0.txt|GNU AFFERO GENERAL PUBLIC LICENSE"
  "licenses/LICENSE-EXCEPTION.md|# Additional permission under AGPL-3.0 section 7"
  "licenses/GPL-3.0.txt|GNU GENERAL PUBLIC LICENSE"
  "licenses/Apache-2.0.txt|Apache License"
)
for entry in "${required_notices[@]}"; do
  rel="${entry%%|*}"; title="${entry#*|}"; f="$app/Contents/Resources/$rel"
  if [[ ! -s "$f" ]]; then
    fail "Contents/Resources/$rel is missing or empty (license notices must ship in the app)"
  elif ! head -n 5 "$f" | grep -q -F "$title"; then
    fail "Contents/Resources/$rel does not start with '$title'"
  else
    ok "Contents/Resources/$rel"
  fi
done

# ADR 0231 amendment 1: Agent Dash's network helper is dev-only until Bozniak clears dash for release. Any file of
# that name anywhere in the bundle fails, signed or not, executable or not.
while IFS= read -r -d '' f; do
  fail "${f#"$app"/}: qrtz-dash-net (Agent Dash's network helper) must not ship in a release bundle (ADR 0231)"
done < <(find "$app" -name 'qrtz-dash-net*' -print0)
# Bozniak M5: a renamed copy is caught too. Every file is grepped for the marker compiled into the helper
# (qrtz_dash_net::BUNDLE_MARKER, kept equal by a test). A compressed copy hides the marker, so no archive may ship
# (by magic bytes, whatever its name) unless it is in archive_allow, and every Mach-O's signing identifier (below) must
# be the one allowed for its path.
dash_marker="qrtz-dash-net:adr-0231:never-in-a-release-bundle:5c1e9b"
# Paths (relative to the .app) of archives a release may carry. Empty: qrtz ships none. Adding one needs @jlgrimes.
archive_allow=()
# Prints the archive kind of a file by its leading bytes (empty if it isn't one).
archive_kind() {
  local h
  h="$(head -c 264 "$1" 2>/dev/null | od -An -tx1 -v | tr -d ' \n')"
  case "$h" in
    1f8b*) echo gzip ;; 1f9d* | 1fa0*) echo compress ;; 504b0304* | 504b0506* | 504b0708*) echo zip ;;
    fd377a585a00*) echo xz ;; 5d0000*) echo lzma ;; 425a68*) echo bzip2 ;; 28b52ffd*) echo zstd ;;
    04224d18*) echo lz4 ;; 377abcaf271c*) echo 7z ;; 52617221*) echo rar ;; 78617221*) echo xar ;;
    303730373031* | 303730373032* | 303730373037* | c771* | 71c7*) echo cpio ;; 213c617263683e*) echo ar ;;
    4c5a4950*) echo lzip ;; 425a5a*) echo bzip ;;
  esac
  [[ "${h:514:10}" == 7573746172 ]] && echo tar # "ustar" at offset 257
}
while IFS= read -r -d '' f; do
  rel="${f#"$app"/}"
  if grep -a -q -F "$dash_marker" "$f" 2>/dev/null; then
    fail "$rel: contains the qrtz-dash-net marker (a renamed copy of Agent Dash's network helper) (ADR 0231)"
  fi
  kind="$(archive_kind "$f" | head -n1)"
  if [[ -n "$kind" ]]; then
    allowed=0
    for a in ${archive_allow[@]+"${archive_allow[@]}"}; do [[ "$rel" == "$a" ]] && allowed=1; done
    ((allowed)) || fail "$rel: $kind archive in the bundle (could hide a compressed copy of any binary) (ADR 0231, M5)"
  fi
done < <(find "$app" -type f -print0)

for bin in "$macos"/*; do
  [[ -f "$bin" ]] || continue
  name="$(basename "$bin")"
  id="$(ident_for "$name")"
  [[ -n "$id" ]] || { fail "$name: unexpected executable in Contents/MacOS"; continue; }
  # Capture first: with pipefail, grep -q exiting early SIGPIPEs codesign.
  info="$(codesign -dv "$bin" 2>&1)" || { fail "$name: unsigned or unreadable signature"; continue; }
  got="$(sed -n 's/^Identifier=//p' <<<"$info")"
  [[ "$got" == "$id" ]] || fail "$name: signing identifier is '$got', want '$id' (was sign-mac.sh run?)"
  req="anchor apple generic and identifier \"$id\" and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = \"$team\""
  if codesign --verify --strict -R="$req" "$bin" 2>/dev/null; then ok "$name satisfies $id, Developer ID, team $team"; else fail "$name does not satisfy the launch requirement ($id, team $team)"; fi
  # QRT-255: a bundled helper must also satisfy the exact requirement its module.json pins (what the launcher checks).
  if [[ -n "$(helper_field "$name" 1)" ]]; then
    if why="$(check_helper "$bin" "$id" "$(helper_field "$name" 4)")"; then ok "$name satisfies its module.json requirement ($id)"; else fail "$name: $why"; fi
  fi
  keys="$(ent_keys "$bin")"
  want="$(ents_for "$name")"
  [[ "$keys" == *get-task-allow* ]] && fail "$name carries com.apple.security.get-task-allow (a debug signature)"
  if [[ "$keys" == "$want" ]]; then ok "$name entitlements: ${want:-none}"; else fail "$name entitlements must be exactly [${want:-none}], got [${keys:-none}]"; fi
done

# Every Mach-O anywhere in the bundle (by magic bytes, not name or `file`) must be one of the three known executables,
# and (Bozniak M5) its signing identifier must be the one allowed for that path: a helper copied over a known name, or
# signed as anything else, fails.
is_macho() {
  case "$(head -c 4 "$1" 2>/dev/null | od -An -tx1 -v | tr -d ' \n')" in
    feedface | feedfacf | cefaedfe | cffaedfe | cafebabe | bebafeca | cafebabf | bfbafeca) return 0 ;;
  esac
  return 1
}
while IFS= read -r -d '' f; do
  rel="${f#"$app"/}"
  is_macho "$f" || continue
  case "$rel" in
    Contents/MacOS/*) want_id="$(ident_for "${rel#Contents/MacOS/}")" ;;
    *) want_id="" ;;
  esac
  [[ -n "$want_id" ]] || fail "unknown Mach-O in the bundle: $rel"
  if command -v codesign >/dev/null 2>&1; then
    sid="$(codesign -dv "$f" 2>&1 | sed -n 's/^Identifier=//p')"
    case "$sid" in *dash-net* | *dash_net*) fail "$rel: signed as $sid (Agent Dash's network helper) (ADR 0231)" ;; esac
    if [[ -n "$want_id" && "$sid" != "$want_id" ]]; then
      fail "$rel: Mach-O signing identifier '${sid:-unsigned}' is not on the allowlist for this path (want '$want_id')"
    elif [[ -z "$want_id" ]]; then
      fail "$rel: Mach-O signing identifier '${sid:-unsigned}' is not on the allowlist"
    fi
    [[ "$(ent_keys "$f")" == *get-task-allow* ]] && fail "$rel carries com.apple.security.get-task-allow"
  fi
done < <(find "$app" -type f -print0)
while IFS= read -r -d '' d; do
  fail "unknown nested code bundle: ${d#"$app"/}"
done < <(find "$app/Contents" -mindepth 1 \( -name '*.app' -o -name '*.framework' -o -name '*.xpc' -o -name '*.appex' -o -name '*.bundle' -o -name '*.dylib' \) -print0)
[[ "$(ent_keys "$app")" == *get-task-allow* ]] && fail "qrtz.app carries com.apple.security.get-task-allow"

# No symlink in Contents may point outside the bundle (or nowhere).
app_real="$(cd "$app" && pwd -P)"
while IFS= read -r -d '' l; do
  rel="${l#"$app"/}"
  dest="$(realpath "$l" 2>/dev/null)" || { fail "symlink $rel -> $(readlink "$l") does not resolve"; continue; }
  [[ "$dest" == "$app_real"/* ]] || fail "symlink $rel resolves outside the bundle: $dest"
done < <(find "$app/Contents" -type l -print0)

if codesign --verify --deep --strict -R="anchor apple generic and identifier \"ai.qrtz.desktop\" and certificate leaf[subject.OU] = \"$team\" and notarized" "$app" 2>/dev/null; then
  ok "codesign --verify --deep --strict (ai.qrtz.desktop, notarized)"
else
  fail "codesign --verify --deep --strict failed (ai.qrtz.desktop, team $team, notarized)"
fi
gk="$(spctl -a -vv "$app" 2>&1)"
if grep -q 'accepted' <<<"$gk" && grep -q 'source=Notarized Developer ID' <<<"$gk"; then ok "spctl: Notarized Developer ID"; else fail "spctl: $(tr '\n' ' ' <<<"$gk")"; fi
if xcrun stapler validate "$app" >/dev/null 2>&1; then ok "stapler validate"; else fail "stapler validate: no valid stapled ticket"; fi
# dev-local (scripts/dev/dev-local.sh, src/dev_local.rs) is debug-only and behind a non-default feature: a release
# qrtz-app, qrtz-echo and qrtz carry none of its override names or its startup line, nor the legacy split/monolith
# overrides it gates (QRTZ_MODULE_HELPERS, QRTZ_HELPER_BIN; Bozniak, 23d892af review).
for bin in "$macos/qrtz-app" "$macos/qrtz" $(helper_names | sed "s|^|$macos/|"); do
  [[ -f "$bin" ]] || continue
  if LC_ALL=C grep -aEq 'QRTZ_(NANO|SNAP|CTRL|ECHO|HELPER)_BIN|QRTZ_MODULE_HELPERS|QRTZ_DEV_REPO|dev-local: ' "$bin"; then
    fail "${bin##*/} contains dev-local override strings (built with --features dev-local or as debug?)"
  else ok "${bin##*/}: no dev-local override strings"; fi
done

if ((fails)); then echo "verify-bundle: $app is NOT shippable ($fails check(s) failed)" >&2; exit 1; fi
echo "verify-bundle: $app passed"
