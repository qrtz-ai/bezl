#!/usr/bin/env bash
# Mac gates for module confinement: ADR 0144 as amended by ADR 0154 (one qrtz identity; extensions share its grants).
# Prints one PASS / FAIL / SKIP / RETIRED line per gate check and exits 1 if any check FAILs.
# Needs no network: the probe (scripts/modules/gate-probe.swift) is compiled with plain swiftc (Xcode or the Command
# Line Tools; no SwiftPM, no FluidAudio, no package download), signed locally with --timestamp=none, and launched
# through LaunchServices (`open`). Stand-ins: ai.qrtz.gate.host-probe plays qrtz.app (the app's TCC entitlements),
# ai.qrtz.gate.inference-probe a sandboxed inference XPC service, and a bare signed helper binary under
# $QRTZ_GATE_DIR/extensions plays a downloaded extension's helper, which the host spawns. Checks that need the voice
# service or the real bundles SKIP with the reason.
#
# Gates: (c) inference XPC has no network, (d) signing identity, (f) voice service isolation, (g) sideload freshness,
# (h) an extension helper spawned by the host is attributed to qrtz. (a), (b) and (e) were per-module TCC checks and
# are RETIRED by ADR 0154 (Jared chose one qrtz entry in Privacy settings).
#
# Usage: scripts/modules/mac-gate.sh [--interactive]
#   --interactive  the probes may prompt (Accessibility), and the script waits while you grant in System Settings;
#                  without it nothing prompts and a check that needs a grant SKIPs with what to do.
# Environment (all optional):
#   QRTZ_GATE_SIGN_ID   a Developer ID Application identity to sign the probes (else ad-hoc; gate d needs it)
#   QRTZ_GATE_TEAM_ID   that identity's team id (gate d's designated requirement)
#   QRTZ_GATE_DIR       where the probe bundles live (default ~/qrtz-gate; keep it stable so TCC grants persist)
#   QRTZ_MODULES_DIR    where extension packages are installed (default ~/Library/Application Support/ai.qrtz.desktop/modules)
#   QRTZ_VOICE_SERVICE  the voice inference XPC mach service name (default ai.qrtz.inference.voice)
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
interactive=0
[[ "${1:-}" == "--interactive" ]] && interactive=1
fails=0
say() { printf '%-7s %-4s %s\n' "$1" "$2" "$3"; }
pass() { say PASS "$1" "$2"; }
fail() {
  say FAIL "$1" "$2"
  fails=$((fails + 1))
}
skip() { say SKIP "$1" "$2"; }
retired() { say RETIRED "$1" "$2"; }
info() { say INFO "$1" "$2"; }

# ADR 0154: with one qrtz identity, per-module TCC separation is given up by choice, so these can't hold and aren't run.
g_retired() {
  retired "(a)" "per-module Accessibility separation: given up by choice, extensions share qrtz's grants (ADR 0154)"
  retired "(b)" "ctrl can't open the mic: extensions share qrtz's grants, module switches are in-app policy (ADR 0154)"
  retired "(e)" "only ctrl may send Apple Events: extensions share qrtz's grants (ADR 0154)"
}
g_skip_all() {
  g_retired
  for g in c d f g h; do skip "($g)" "$1"; done
}

echo "qrtz module confinement gates (ADR 0144, amended by ADR 0154)"
if [[ "$(uname -s)" != "Darwin" ]]; then
  g_skip_all "not macOS ($(uname -s)); the Linux side is cargo test -p qrtz-lifecycle --test offline_graph"
  exit 0
fi
if ! command -v xcrun >/dev/null || ! xcrun --find swiftc >/dev/null 2>&1; then
  g_skip_all "no swiftc (install the Xcode Command Line Tools: xcode-select --install)"
  exit 0
fi

gate_dir="${QRTZ_GATE_DIR:-$HOME/qrtz-gate}"
modules_dir="${QRTZ_MODULES_DIR:-$HOME/Library/Application Support/ai.qrtz.desktop/modules}"
voice="${QRTZ_VOICE_SERVICE:-ai.qrtz.inference.voice}"
sign_id="${QRTZ_GATE_SIGN_ID:-}"
team="${QRTZ_GATE_TEAM_ID:-}"
mkdir -p "$gate_dir/build"
signing="ad-hoc"
[[ -n "$sign_id" ]] && signing="identity '$sign_id'"

# ---- build the probe once (stable bytes for a stable build tag, so ad-hoc cdhashes and their grants persist) ----
build_probe() { # $1 = build tag, $2 = output binary
  printf 'let probeBuild = "%s"\n' "$1" >"$gate_dir/build/build.swift"
  cp "$here/gate-probe.swift" "$gate_dir/build/main.swift"
  xcrun swiftc -O -swift-version 5 -o "$2" "$gate_dir/build/main.swift" "$gate_dir/build/build.swift" \
    -framework ApplicationServices -framework AVFoundation -framework CoreServices >"$gate_dir/build/swiftc.log" 2>&1
}
if ! build_probe "m0" "$gate_dir/build/gate-probe"; then
  cat "$gate_dir/build/swiftc.log"
  g_skip_all "the probe did not compile (log above)"
  exit 1
fi

# ---- stand-in bundles: one bundle id, one entitlements set and one signature each -------------------------------
plist_bool_dict() { # key... -> plist with each key true
  printf '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict>\n'
  for k in "$@"; do printf '  <key>%s</key><true/>\n' "$k"; done
  printf '</dict></plist>\n'
}
make_bundle() { # $1 = role (host|inference), $2.. = entitlements set true
  local role="$1" id="ai.qrtz.gate.$1-probe"
  shift
  local app="$gate_dir/$id.app"
  mkdir -p "$app/Contents/MacOS"
  cp "$gate_dir/build/gate-probe" "$app/Contents/MacOS/gate-probe"
  cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>$id</string>
  <key>CFBundleName</key><string>qrtz gate $role probe</string>
  <key>CFBundleExecutable</key><string>gate-probe</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.1</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSBackgroundOnly</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>qrtz gate probe (ADR 0154): checks whether this bundle and its extension helpers may use the microphone.</string>
  <key>NSAppleEventsUsageDescription</key><string>qrtz gate probe (ADR 0154): checks whether this bundle may send Apple Events.</string>
</dict></plist>
PLIST
  # Same usage strings in every probe, so a refusal comes from the entitlements, not a missing string.
  plist_bool_dict "$@" >"$gate_dir/build/$id.entitlements"
  sign "$app" "$gate_dir/build/$id.entitlements"
}
sign() { # $1 = bundle, $2 = entitlements
  if [[ -n "$sign_id" ]]; then
    codesign --force --timestamp=none --options runtime --entitlements "$2" -s "$sign_id" "$1" >/dev/null 2>&1
  else
    codesign --force --timestamp=none --options runtime --entitlements "$2" -s - "$1" >/dev/null 2>&1
  fi
}
# The host carries the app's TCC entitlements allowlist (confinement-map.json appTccEntitlements).
make_bundle host com.apple.security.device.audio-input com.apple.security.automation.apple-events || {
  g_skip_all "codesign failed"
  exit 1
}
make_bundle inference com.apple.security.app-sandbox
# A downloaded extension's helper: a bare binary outside the host bundle, with its own signing identifier and the
# hardened-runtime entitlements its process needs (entitlements are per process; TCC grants are qrtz's, ADR 0154).
ext_dir="$gate_dir/extensions/echo"
ext_bin="$ext_dir/ai.qrtz.gate.ext-echo"
ext_id="ai.qrtz.gate.ext-echo"
mkdir -p "$ext_dir"
cp "$gate_dir/build/gate-probe" "$ext_bin"
plist_bool_dict com.apple.security.device.audio-input >"$gate_dir/build/$ext_id.entitlements"
if [[ -n "$sign_id" ]]; then
  codesign --force --timestamp=none --options runtime -i "$ext_id" --entitlements "$gate_dir/build/$ext_id.entitlements" -s "$sign_id" "$ext_bin" >/dev/null 2>&1
else
  codesign --force --timestamp=none --options runtime -i "$ext_id" --entitlements "$gate_dir/build/$ext_id.entitlements" -s - "$ext_bin" >/dev/null 2>&1
fi
info "" "probes signed $signing, hardened runtime, in $gate_dir"

# Run a probe through LaunchServices and print its result line. $1 = role, rest = probe args (a `--` and what follows
# go to a spawned child, so the probe's own --out is put before them).
probe() {
  local role="$1" id="ai.qrtz.gate.$1-probe"
  shift
  local out="$gate_dir/build/$role.out"
  [[ "$role" == "inference" ]] && out="$HOME/Library/Containers/$id/Data/qrtz-gate-probe.out"
  rm -f "$out"
  local extra=()
  [[ "$role" != "inference" ]] && extra=(--out "$out")
  # ${a[@]+"${a[@]}"}: macOS's bash 3.2 treats an empty array as unset under set -u.
  open -n -W -g -a "$gate_dir/$id.app" --args ${extra[@]+"${extra[@]}"} "$@" >/dev/null 2>&1
  [[ -f "$out" ]] && cat "$out" || echo "probe $1 error=no-output"
}
field() { # $1 = line, $2 = key
  sed -nE "s/.* $2=([^ ]*).*/\\1/p" <<<"$1"
}
wait_user() {
  ((interactive)) || return 1
  read -r -p "  -> $1 Press Enter when done. " _
}

# Installed extensions, if any (INFO only: their bundle ids and DRs are package signing identities, not TCC ones).
for m in echo nano ctrl; do
  b="$(find "$modules_dir" -maxdepth 4 -name "ai.qrtz.$m*" -print -quit 2>/dev/null)"
  if [[ -n "$b" ]]; then
    info "" "found $b; entitlements: $(codesign -d --entitlements - --xml "$b" 2>/dev/null | tr -d '\n' | grep -oE '<key>[^<]+</key><true/>' | sed -E 's/<key>([^<]+).*/\1/' | tr '\n' ' ')"
  else
    info "" "no installed $m extension under $modules_dir: gates run on the stand-in probes"
  fi
done

# ---- (h) an extension helper spawned by the host is attributed to qrtz -------------------------------------------
# ADR 0154: qrtz spawns an extension's helper (no LaunchServices), so TCC charges the helper's requests to qrtz: it
# inherits qrtz's grant and gets no Privacy entry of its own. The helper has its own signing identifier and has never
# been granted anything, so trusted=true in the child can only come from the host's grant.
gate_h() {
  local h
  h="$(probe host ax)"
  if [[ "$(field "$h" trusted)" != "true" ]]; then
    probe host ax --ask >/dev/null
    if ! wait_user "Grant Accessibility to 'qrtz gate host probe' (System Settings > Privacy & Security > Accessibility)."; then
      skip "(h)" "the host probe has no Accessibility; grant it to 'qrtz gate host probe', or run with --interactive"
      return
    fi
    h="$(probe host ax)"
    if [[ "$(field "$h" trusted)" != "true" ]]; then
      skip "(h)" "the host probe still has no Accessibility (trusted=$(field "$h" trusted)); grant it and rerun"
      return
    fi
  fi
  # Make sure the helper's own identity holds nothing (a no-op when it was never granted).
  tccutil reset Accessibility "$ext_id" >/dev/null 2>&1
  local out="$gate_dir/build/ext-echo.out" s c
  rm -f "$out"
  s="$(probe host spawn "$ext_bin" -- ax --out "$out")"
  if [[ "$(field "$s" child_exit)" != "0" || ! -f "$out" ]]; then
    fail "(h)" "the host couldn't run the extension helper: $s"
    return
  fi
  c="$(cat "$out")"
  if [[ "$(field "$c" trusted)" == "true" ]]; then
    pass "(h)" "extension helper ($ext_id, outside the host bundle) spawned by the host: AXIsProcessTrusted=true from qrtz's grant, no Privacy entry of its own ($signing)"
  else
    fail "(h)" "extension helper spawned by the granted host is not trusted ($c): it's not attributed to qrtz, so ADR 0154's single-entry model needs a launcher change"
  fi
  # Mic: the same attribution (INFO only, nothing prompts): the child's status should match the host's.
  rm -f "$out"
  probe host spawn "$ext_bin" -- mic --out "$out" >/dev/null
  info "(h)" "mic status: host=$(field "$(probe host mic)" status) extension helper=$(field "$(cat "$out" 2>/dev/null)" status) (should match)"
  # Contrast (not a gate): the same helper run from this shell is attributed to the terminal, not to qrtz.
  info "(h)" "run from this shell instead (attributed to the terminal): $("$ext_bin" ax --out /dev/null | tr -d '\n')"
}

# ---- (c) an inference XPC service has no network -------------------------------------------------------------------
gate_c() {
  local i e port=9
  i="$(probe inference socket "$port")"
  e="$(probe host socket "$port")"
  local it iu
  it="$(field "$i" tcp)"
  iu="$(field "$i" udp)"
  if [[ -z "$it" ]]; then
    fail "(c)" "no sandboxed probe result: $i"
  elif [[ "$it" =~ ^(EPERM|EACCES|socket-EPERM|socket-EACCES)$ && "$iu" =~ ^(EPERM|EACCES|socket-EPERM|socket-EACCES)$ ]]; then
    pass "(c)" "sandboxed stand-in (app-sandbox, no network.client): loopback tcp=$it udp=$iu; unsandboxed host probe got tcp=$(field "$e" tcp)"
  else
    fail "(c)" "sandboxed stand-in reached the network stack: tcp=$it udp=$iu"
  fi
  local x
  x="$(find "$modules_dir" -maxdepth 6 -name 'ai.qrtz.inference.*.xpc' -print -quit 2>/dev/null)"
  if [[ -z "$x" ]]; then
    skip "(c)" "real inference XPC service: none installed yet (M1/M2); then this checks its signed entitlements too"
  else
    local ent
    ent="$(codesign -d --entitlements - --xml "$x" 2>/dev/null)"
    if grep -qE 'security\.(network|device|personal-information|automation)\.' <<<"$ent"; then
      fail "(c)" "$x holds a network or TCC entitlement"
    elif grep -q 'com.apple.security.app-sandbox' <<<"$ent"; then
      pass "(c)" "$x: sandboxed, no network or TCC entitlement"
    else
      fail "(c)" "$x is not sandboxed"
    fi
  fi
}

# ---- (d) signing identity: other team refused, same-team update keeps grants -----------------------------------------
gate_d() {
  if [[ -z "$sign_id" || -z "$team" ]]; then
    skip "(d)" "set QRTZ_GATE_SIGN_ID and QRTZ_GATE_TEAM_ID (Developer ID) to check the designated requirement and grant survival"
  else
    local req="anchor apple generic and certificate leaf[subject.OU] = \"$team\" and identifier \"ai.qrtz.gate.host-probe\""
    local app="$gate_dir/ai.qrtz.gate.host-probe.app" other="$gate_dir/build/other-team.app"
    rm -rf "$other"
    cp -R "$app" "$other"
    codesign --force --timestamp=none --options runtime -s - "$other" >/dev/null 2>&1
    if codesign --verify -R="$req" "$app" 2>/dev/null && ! codesign --verify -R="$req" "$other" 2>/dev/null; then
      pass "(d)" "the team requirement accepts our signature and refuses the same bundle re-signed by someone else"
    else
      fail "(d)" "designated requirement check: ours vs re-signed did not split"
    fi
    rm -rf "$other"
    # Same-team update: new bytes, same identity; the host's AX grant from (h) should survive.
    if [[ "$(field "$(probe host ax)" trusted)" == "true" ]]; then
      build_probe "m0-update-$(date +%s)" "$gate_dir/build/gate-probe-v2" &&
        cp "$gate_dir/build/gate-probe-v2" "$app/Contents/MacOS/gate-probe" &&
        sign "$app" "$gate_dir/build/ai.qrtz.gate.host-probe.entitlements"
      local after
      after="$(probe host ax)"
      if [[ "$(field "$after" trusted)" == "true" ]]; then
        pass "(d)" "same-team update kept the Accessibility grant (build $(field "$after" build))"
      else
        fail "(d)" "same-team update lost the Accessibility grant"
      fi
    else
      skip "(d)" "grant survival needs the host probe granted Accessibility first (gate h)"
    fi
  fi
  skip "(d)" "installer refuses another team's package: covered by the installer's tests (crates/qrtz-app/tests/module_hosting.rs), not run here"
}

# ---- (f) voice service: undeclared and unsigned clients refused, no cross-session reads -----------------------------
gate_f() {
  if ! launchctl print "gui/$(id -u)/$voice" >/dev/null 2>&1; then
    skip "(f)" "the voice inference service $voice isn't installed (M2); then: an unsigned client and an undeclared client are refused"
  else
    local r
    r="$(probe host xpc "$voice")"
    # The stand-in's signature declares no dependency on the voice service, so it must be refused.
    if [[ "$(field "$r" result)" == refused-* ]]; then
      pass "(f)" "undeclared client refused by $voice: $(field "$r" result)"
    else
      fail "(f)" "undeclared client got a reply from $voice"
    fi
  fi
  skip "(f)" "nano can't read an in-flight echo session: needs the echo and nano modules on the voice service (M2)"
}

# ---- (g) sideload freshness ---------------------------------------------------------------------------------------
gate_g() {
  skip "(g)" "sideload freshness is installer logic, not a Mac property: cargo test -p qrtz-publish --test gate_g runs ADR 0144's four cases on ADR 0157's installer (expired-list sideload installs marked stale, revoked id refused before the prompt, older snapshot refused, later fresh index disables a revoked module) plus the freshness exemption"
}

g_retired
gate_c
gate_h
gate_d
gate_f
gate_g
if ((fails)); then
  echo "$fails gate check(s) FAILED."
  echo "If (h) failed, extension helpers aren't attributed to qrtz: ADR 0154's single Privacy entry needs a launcher change."
  exit 1
fi
echo "no gate check failed (SKIPs say what is still missing)"
