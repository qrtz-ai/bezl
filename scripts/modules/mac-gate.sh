#!/usr/bin/env bash
# ADR 0144 Mac gates (a)-(g) for module confinement. Prints one PASS / FAIL / SKIP line per gate check and exits 1
# if any check FAILs. Needs no network: the probe (scripts/modules/gate-probe.swift) is compiled with plain swiftc
# (Xcode or the Command Line Tools; no SwiftPM, no FluidAudio, no package download), signed locally with
# --timestamp=none, and launched through LaunchServices (`open`) so TCC attributes it to its own bundle, as it will
# the M1 module apps. Until M1 the gates run against stand-in bundles (ai.qrtz.gate.{echo,ctrl,inference}-probe)
# that carry ADR 0144's per-module entitlements; checks that need the real module bundles, the voice service or the
# installer SKIP with the reason.
#
# Usage: scripts/modules/mac-gate.sh [--interactive]
#   --interactive  the probes may prompt (Accessibility, Automation), and the script waits while you grant in
#                  System Settings; without it nothing prompts and a check that needs a grant SKIPs with what to do.
# Environment (all optional):
#   QRTZ_GATE_SIGN_ID   a Developer ID Application identity to sign the probes (else ad-hoc; gate d needs it)
#   QRTZ_GATE_TEAM_ID   that identity's team id (gate d's designated requirement)
#   QRTZ_GATE_DIR       where the probe bundles live (default ~/qrtz-gate; keep it stable so TCC grants persist)
#   QRTZ_MODULES_DIR    where M1 module bundles are installed (default ~/Library/Application Support/ai.qrtz.desktop/modules)
#   QRTZ_VOICE_SERVICE  the voice inference XPC mach service name (default ai.qrtz.inference.voice)
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
interactive=0
[[ "${1:-}" == "--interactive" ]] && interactive=1
fails=0
say() { printf '%-5s %-4s %s\n' "$1" "$2" "$3"; }
pass() { say PASS "$1" "$2"; }
fail() {
  say FAIL "$1" "$2"
  fails=$((fails + 1))
}
skip() { say SKIP "$1" "$2"; }
info() { say INFO "$1" "$2"; }

g_skip_all() {
  for g in a b c d e f g; do skip "($g)" "$1"; done
}

echo "qrtz module confinement gates (ADR 0144)"
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
make_bundle() { # $1 = role (echo|ctrl|inference), $2.. = entitlements set true
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
  <key>NSMicrophoneUsageDescription</key><string>ADR 0144 gate probe: checks whether this bundle may use the microphone.</string>
  <key>NSAppleEventsUsageDescription</key><string>ADR 0144 gate probe: checks whether this bundle may send Apple Events.</string>
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
make_bundle echo com.apple.security.device.audio-input || {
  g_skip_all "codesign failed"
  exit 1
}
make_bundle ctrl com.apple.security.automation.apple-events
make_bundle inference com.apple.security.app-sandbox
info "" "probes signed $signing, hardened runtime, in $gate_dir"

# Run a probe through LaunchServices and print its result line. $1 = role, rest = probe args.
probe() {
  local role="$1" id="ai.qrtz.gate.$1-probe"
  shift
  local out="$gate_dir/build/$role.out"
  [[ "$role" == "inference" ]] && out="$HOME/Library/Containers/$id/Data/qrtz-gate-probe.out"
  rm -f "$out"
  local extra=()
  [[ "$role" != "inference" ]] && extra=(--out "$out")
  # ${a[@]+"${a[@]}"}: macOS's bash 3.2 treats an empty array as unset under set -u.
  open -n -W -g -a "$gate_dir/$id.app" --args "$@" ${extra[@]+"${extra[@]}"} >/dev/null 2>&1
  [[ -f "$out" ]] && cat "$out" || echo "probe $1 error=no-output"
}
field() { # $1 = line, $2 = key
  sed -nE "s/.* $2=([^ ]*).*/\\1/p" <<<"$1"
}
wait_user() {
  ((interactive)) || return 1
  read -r -p "  -> $1 Press Enter when done. " _
}

# Real module bundles, when M1 has installed them (Bobs's launcher defines their probe mode; until then INFO only).
for m in echo nano ctrl; do
  b="$(find "$modules_dir" -maxdepth 4 -name "ai.qrtz.$m.app" -print -quit 2>/dev/null)"
  if [[ -n "$b" ]]; then
    info "" "found $b; entitlements: $(codesign -d --entitlements - --xml "$b" 2>/dev/null | tr -d '\n' | grep -oE '<key>[^<]+</key><true/>' | sed -E 's/<key>([^<]+).*/\1/' | tr '\n' ' ')"
  else
    info "" "no M1 bundle ai.qrtz.$m.app under $modules_dir: gates run on the stand-in probes"
  fi
done

# ---- (a) Accessibility is per bundle ------------------------------------------------------------------------------
gate_a() {
  local e c
  e="$(probe echo ax)"
  c="$(probe ctrl ax)"
  if [[ "$(field "$c" trusted)" != "true" && "$(field "$e" trusted)" != "true" ]]; then
    probe ctrl ax --ask >/dev/null
    if ! wait_user "Grant Accessibility to 'qrtz gate ctrl probe' only (System Settings > Privacy & Security > Accessibility)."; then
      skip "(a)" "no probe has Accessibility; grant it to 'qrtz gate ctrl probe' only, or run with --interactive"
      return
    fi
    e="$(probe echo ax)"
    c="$(probe ctrl ax)"
  fi
  local et ct
  et="$(field "$e" trusted)"
  ct="$(field "$c" trusted)"
  if [[ "$ct" == "true" && "$et" != "true" ]]; then
    pass "(a)" "only ctrl granted AX: ctrl AXIsProcessTrusted=true, echo=false (stand-in, $signing)"
  elif [[ "$et" == "true" && "$ct" != "true" ]]; then
    pass "(a)" "only echo granted AX: echo AXIsProcessTrusted=true, ctrl=false (stand-in, $signing)"
  elif [[ "$et" == "true" && "$ct" == "true" ]]; then
    skip "(a)" "both probes are granted; remove one in System Settings (or: tccutil reset Accessibility ai.qrtz.gate.echo-probe) and rerun"
    return
  else
    skip "(a)" "neither probe has Accessibility yet (echo=$et ctrl=$ct); grant 'qrtz gate ctrl probe' only and rerun"
    return
  fi
  # Vice versa: move the grant to the other probe.
  if ((interactive)) && [[ "$ct" == "true" ]]; then
    tccutil reset Accessibility ai.qrtz.gate.ctrl-probe >/dev/null 2>&1
    probe echo ax --ask >/dev/null
    wait_user "Now grant Accessibility to 'qrtz gate echo probe' (ctrl's grant was reset)."
    e="$(probe echo ax)"
    c="$(probe ctrl ax)"
    if [[ "$(field "$e" trusted)" == "true" && "$(field "$c" trusted)" != "true" ]]; then
      pass "(a)" "vice versa: echo=true, ctrl=false"
    else
      fail "(a)" "vice versa: echo=$(field "$e" trusted) ctrl=$(field "$c" trusted)"
    fi
  else
    skip "(a)" "vice versa needs --interactive (moves the grant to the other probe)"
  fi
  # Contrast (not a gate): the same binary spawned from this shell is attributed to the terminal, not to itself.
  info "(a)" "spawned from this shell instead (attributed to the terminal): $("$gate_dir/ai.qrtz.gate.echo-probe.app/Contents/MacOS/gate-probe" ax --out /dev/null | tr -d '\n')"
}

# ---- (b) ctrl can't open the mic -----------------------------------------------------------------------------------
gate_b() {
  local c
  c="$(probe ctrl mic --open --ask)"
  if [[ "$(field "$c" opened)" == "yes" || "$(field "$c" status_after)" == "authorized" ]]; then
    fail "(b)" "ctrl opened the mic: $c"
  elif [[ -z "$(field "$c" opened)" ]]; then
    fail "(b)" "no probe result: $c"
  else
    pass "(b)" "ctrl mic open denied (no audio-input entitlement): granted=$(field "$c" granted) opened=$(field "$c" opened) status=$(field "$c" status_after)"
  fi
  info "(b)" "echo probe mic status (not asked): $(field "$(probe echo mic)" status)"
}

# ---- (c) an inference XPC service has no network -------------------------------------------------------------------
gate_c() {
  local i e port=9
  i="$(probe inference socket "$port")"
  e="$(probe echo socket "$port")"
  local it iu
  it="$(field "$i" tcp)"
  iu="$(field "$i" udp)"
  if [[ -z "$it" ]]; then
    fail "(c)" "no sandboxed probe result: $i"
  elif [[ "$it" =~ ^(EPERM|EACCES|socket-EPERM|socket-EACCES)$ && "$iu" =~ ^(EPERM|EACCES|socket-EPERM|socket-EACCES)$ ]]; then
    pass "(c)" "sandboxed stand-in (app-sandbox, no network.client): loopback tcp=$it udp=$iu; unsandboxed echo probe got tcp=$(field "$e" tcp)"
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
    local req="anchor apple generic and certificate leaf[subject.OU] = \"$team\" and identifier \"ai.qrtz.gate.ctrl-probe\""
    local app="$gate_dir/ai.qrtz.gate.ctrl-probe.app" other="$gate_dir/build/other-team.app"
    rm -rf "$other"
    cp -R "$app" "$other"
    codesign --force --timestamp=none --options runtime -s - "$other" >/dev/null 2>&1
    if codesign --verify -R="$req" "$app" 2>/dev/null && ! codesign --verify -R="$req" "$other" 2>/dev/null; then
      pass "(d)" "the team requirement accepts our signature and refuses the same bundle re-signed by someone else"
    else
      fail "(d)" "designated requirement check: ours vs re-signed did not split"
    fi
    rm -rf "$other"
    # Same-team update: new bytes, same identity; the AX grant from (a) should survive.
    if [[ "$(field "$(probe ctrl ax)" trusted)" == "true" ]]; then
      build_probe "m0-update-$(date +%s)" "$gate_dir/build/gate-probe-v2" &&
        cp "$gate_dir/build/gate-probe-v2" "$app/Contents/MacOS/gate-probe" &&
        sign "$app" "$gate_dir/build/ai.qrtz.gate.ctrl-probe.entitlements"
      local after
      after="$(probe ctrl ax)"
      if [[ "$(field "$after" trusted)" == "true" ]]; then
        pass "(d)" "same-team update kept the Accessibility grant (build $(field "$after" build))"
      else
        fail "(d)" "same-team update lost the Accessibility grant"
      fi
    else
      skip "(d)" "grant survival needs the ctrl probe granted Accessibility first (gate a)"
    fi
  fi
  skip "(d)" "installer refuses another team's module and keeps grants on update: the installer doesn't exist yet (M1/M2)"
}

# ---- (e) Apple Events: ctrl yes, echo no ------------------------------------------------------------------------------
gate_e() {
  open -g -b com.apple.systemevents >/dev/null 2>&1
  sleep 1
  local c e
  local ask=()
  ((interactive)) && ask=(--ask)
  c="$(probe ctrl ae ${ask[@]+"${ask[@]}"})"
  e="$(probe echo ae --ask)"
  local ctrl_perm ep
  ctrl_perm="$(field "$c" permission)"
  ep="$(field "$e" permission)"
  if [[ "$(field "$e" sent)" == "ok" ]]; then
    fail "(e)" "echo (no apple-events entitlement) automated System Events: $e"
    return
  fi
  if [[ "$ctrl_perm" == "-1744" ]]; then
    skip "(e)" "ctrl's Automation isn't approved yet; run with --interactive to approve it (echo refused: permission=$ep)"
  elif [[ "$(field "$c" sent)" == "ok" ]]; then
    pass "(e)" "ctrl's Apple Event to System Events succeeded; echo's refused (permission=$ep) ($signing, hardened runtime)"
  else
    fail "(e)" "ctrl's Apple Event failed: $c"
  fi
}

# ---- (f) voice service: undeclared and unsigned clients refused, no cross-session reads -----------------------------
gate_f() {
  if ! launchctl print "gui/$(id -u)/$voice" >/dev/null 2>&1; then
    skip "(f)" "the voice inference service $voice isn't installed (M2); then: an unsigned client and an undeclared client are refused"
  else
    local r
    r="$(probe echo xpc "$voice")"
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
  skip "(g)" "sideload freshness: the installer doesn't exist yet; ADR 0144 lists its four cases (expired-list sideload installs, revoked id refused, older snapshot refused, later fresh index disables a revoked module)"
}

gate_a
gate_b
gate_c
gate_d
gate_e
gate_f
gate_g
if ((fails)); then
  echo "$fails gate check(s) FAILED."
  echo "If (a) failed, ADR 0144's per-module identity plan is void: stop the module split and report it."
  exit 1
fi
echo "no gate check failed (SKIPs say what is still missing)"
