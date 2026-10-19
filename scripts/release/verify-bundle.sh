#!/usr/bin/env bash
# The release gate (0.2.4 post-mortem): a qrtz.app is shippable or installable only if this passes. Shared by
# sign-notarize.sh (release:mac, after notarize and staple), scripts/app/publish-local.sh (before upload) and
# .github/workflows/app-release.yml, so CI and local check the same thing.
#
# Checks, each failing nonzero with a message:
#   - every executable in Contents/MacOS satisfies the requirement a release build launches it under
#     (crates/qrtz-app/src/installed.rs launch_requirement): qrtz-app ai.qrtz.desktop, qrtz-echo ai.qrtz.echo,
#     qrtz ai.qrtz.cli, Developer ID, team MR26CCZ496; an unknown executable fails;
#   - qrtz-echo's entitlements are exactly audio-input; calendars appear only on qrtz-app;
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

ident_for() { case "$1" in qrtz-app) echo ai.qrtz.desktop ;; qrtz-echo) echo ai.qrtz.echo ;; qrtz) echo ai.qrtz.cli ;; *) echo "" ;; esac; }
ent_keys() { codesign -d --entitlements - --xml "$1" 2>/dev/null | plutil -convert xml1 -o - - 2>/dev/null | grep -o '<key>[^<]*</key>' | sed 's/<[^>]*>//g' | sort | tr '\n' ' ' | sed 's/ $//'; }

for name in qrtz-app qrtz-echo qrtz; do [[ -x "$macos/$name" ]] || fail "$name is missing from Contents/MacOS"; done

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
  keys="$(ent_keys "$bin")"
  case "$name" in
    qrtz-echo) [[ "$keys" == "com.apple.security.device.audio-input" ]] && ok "qrtz-echo entitlements: audio-input only" || fail "qrtz-echo entitlements must be audio-input only, got: ${keys:-none}" ;;
    qrtz-app) : ;;
    *) [[ "$keys" == *personal-information.calendars* ]] && fail "$name carries the calendars entitlement (only qrtz-app may)" ;;
  esac
done

if codesign --verify --deep --strict -R="anchor apple generic and identifier \"ai.qrtz.desktop\" and certificate leaf[subject.OU] = \"$team\" and notarized" "$app" 2>/dev/null; then
  ok "codesign --verify --deep --strict (ai.qrtz.desktop, notarized)"
else
  fail "codesign --verify --deep --strict failed (ai.qrtz.desktop, team $team, notarized)"
fi
gk="$(spctl -a -vv "$app" 2>&1)"
if grep -q 'accepted' <<<"$gk" && grep -q 'source=Notarized Developer ID' <<<"$gk"; then ok "spctl: Notarized Developer ID"; else fail "spctl: $(tr '\n' ' ' <<<"$gk")"; fi
if xcrun stapler validate "$app" >/dev/null 2>&1; then ok "stapler validate"; else fail "stapler validate: no valid stapled ticket"; fi

if ((fails)); then echo "verify-bundle: $app is NOT shippable ($fails check(s) failed)" >&2; exit 1; fi
echo "verify-bundle: $app passed"
