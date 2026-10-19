#!/usr/bin/env bash
# Mac gates for module confinement: ADR 0144 as amended by ADR 0154 (one qrtz identity; extensions share its grants).
# Prints one PASS / FAIL / SKIP / RETIRED line per gate check and exits 1 if any check FAILs.
# Needs no network: the probe (scripts/modules/gate-probe.swift) is compiled with plain swiftc (Xcode or the Command
# Line Tools; no SwiftPM, no FluidAudio, no package download), signed locally with --timestamp=none, and launched
# through LaunchServices (`open`). Stand-ins: ai.qrtz.gate.host-probe plays qrtz.app (the app's TCC entitlements),
# and a bare signed helper binary under
# $QRTZ_GATE_DIR/extensions plays a downloaded extension's helper, which the host spawns. Gate (h) then runs on every
# module the real installer put under qrtz's data dir (ADR 0169: qrtz ships empty; install one from the marketplace or
# with `qrtz install snap`): its qrtz.lock entry, its Developer ID requirement for team MR26CCZ496, its notarized cdhash
# and its read-only package dir must all hold, so a locally built helper never stands in for it. Gate (h) also runs
# bundled echo (ADR 0174: qrtz.app/Contents/MacOS/qrtz-echo, no lock entry) once it is a plain file in the signed app
# satisfying its Developer ID requirement for ai.qrtz.echo, team MR26CCZ496. Checks that need a real bundle SKIP with
# the reason.
#
# Gates: (d) signing identity, (g) sideload freshness, (h) an extension helper spawned by the host is attributed to
# qrtz. (a), (b) and (e) were per-module TCC checks and are RETIRED by ADR 0154 (Jared chose one qrtz entry in Privacy
# settings); (c) and (f) checked the inference XPC services, which ADR 0174 cut from v1 (RETIRED).
#
# Usage: scripts/modules/mac-gate.sh [--interactive]
#   --interactive  the probes may prompt (Accessibility), and the script waits while you grant in System Settings;
#                  without it nothing prompts and a check that needs a grant SKIPs with what to do.
# Environment (all optional):
#   QRTZ_GATE_SIGN_ID   the identity that signs the probes (default "Developer ID Application: Jared Grimes
#                       (MR26CCZ496)"; ad-hoc when it isn't in the keychain, or when set to the empty string)
#   QRTZ_GATE_TEAM_ID   that identity's team id (default MR26CCZ496; gate d's and gate h's designated requirement)
#   QRTZ_GATE_DIR       where the probe bundles live (default ~/qrtz-gate; keep it stable so TCC grants persist)
#   QRTZ_DATA_DIR       qrtz's data dir, where qrtz.lock and packages/ are (default QRTZ_USER_DATA, else
#                       ~/Library/Application Support/ai.qrtz.desktop)
#   QRTZ_APP            the signed qrtz.app whose bundled echo gate (h) runs (default /Applications/qrtz.app)
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
  retired "(c)" "inference XPC has no network: no inference XPC service ships in v1 (inference.voice cut, ADR 0174)"
  retired "(f)" "voice service isolation: no voice inference service in v1 (inference.voice cut, ADR 0174)"
}
g_skip_all() {
  g_retired
  for g in d g h; do skip "($g)" "$1"; done
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
data_dir="${QRTZ_DATA_DIR:-${QRTZ_USER_DATA:-$HOME/Library/Application Support/ai.qrtz.desktop}}"
qrtz_app="${QRTZ_APP:-/Applications/qrtz.app}"
# Jared's Developer ID (ADR 0170: the team every release path pins). `${VAR-default}`: set to "" for ad-hoc.
release_team="MR26CCZ496"
sign_id="${QRTZ_GATE_SIGN_ID-Developer ID Application: Jared Grimes ($release_team)}"
team="${QRTZ_GATE_TEAM_ID-$release_team}"
if [[ -n "$sign_id" ]] && ! security find-identity -v -p codesigning 2>/dev/null | grep -qF "\"$sign_id\""; then
  info "" "signing identity '$sign_id' isn't in this keychain: the probes are signed ad-hoc (gate d SKIPs)"
  sign_id=""
fi
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
make_bundle() { # $1 = role (host), $2.. = entitlements set true
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
  rm -f "$out"
  local extra=(--out "$out")
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

# ---- modules the real installer put under the data dir (ADR 0169) ----------------------------------------------------
# `installed_module <id>` prints `<helper path>\t<version>\t<source>\t<designated requirement>\t<notarized cdhash>`
# from qrtz.lock and the package's own qrtz.json (an installed, not disabled extension), or nothing. JavaScript for
# Automation reads the JSON: nothing to install on a stock Mac.
installed_module() {
  osascript -l JavaScript - "$data_dir" "$1" 2>/dev/null <<'JS'
function run(argv) {
  ObjC.import("Foundation");
  const read = (p) => {
    const s = $.NSString.stringWithContentsOfFileEncodingError(p, $.NSUTF8StringEncoding, null);
    return s.isNil() ? null : JSON.parse(s.js);
  };
  const [data, id] = argv;
  const lock = read(data + "/qrtz.lock");
  const v = lock && lock.workspace && lock.workspace.resolved && lock.workspace.resolved[id];
  const l = v && lock.packages[id + "@" + v];
  if (!l || l.kind !== "extension" || l.disabled) return "";
  const dir = data + "/packages/" + id + "/" + v;
  const m = read(dir + "/qrtz.json");
  if (!m || !m.extension || !m.extension.entry) return "";
  const n = l.notarization || {};
  return [dir + "/" + m.extension.entry, v, l.source || "", l.designatedRequirement || "", n.cdhash || ""].join("\t");
}
JS
}
# The Developer ID requirement the installer and the launcher pin for `ai.qrtz.<id>` (qrtz_modules::manifest).
module_requirement() { # $1 = id
  printf 'identifier "ai.qrtz.%s" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = "%s"' "$1" "$release_team"
}
# Why `<path>` isn't a module the real installer put there (empty = it is): a network or sideload install, the lock's
# requirement is the release one, the file satisfies it, its cdhash is the one the signed index says Apple notarized,
# no quarantine, and the package dir is read-only (what the installer leaves; a build-packages.sh output isn't).
not_installed_by_qrtz() { # $1 = id, $2 = path, $3 = source, $4 = lock requirement, $5 = notarized cdhash
  local want cd
  want="$(module_requirement "$1")"
  [[ "$3" == "network" || "$3" == "sideload" ]] || { echo "lock source '$3' (not a marketplace install)"; return; }
  [[ "$4" == "$want" ]] || { echo "the lock's requirement isn't the Developer ID one for team $release_team"; return; }
  [[ -x "$2" ]] || { echo "$2 is missing"; return; }
  codesign --verify --strict "-R=$want" "$2" >/dev/null 2>&1 || { echo "$2 doesn't satisfy the release requirement"; return; }
  cd="$(codesign -d -vvv "$2" 2>&1 | sed -nE 's/^CDHash=([0-9a-f]+)$/\1/p')"
  [[ -n "$5" && "$cd" == "$5" ]] || { echo "cdhash $cd isn't the notarized ${5:-<none>}"; return; }
  xattr -p com.apple.quarantine "$2" >/dev/null 2>&1 && { echo "$2 is quarantined"; return; }
  [[ -w "$(dirname "$2")" || -w "$2" ]] && { echo "its package dir is writable (not the installer's)"; return; }
  true
}
real_modules=() # "<id>\t<path>\t<version>" for each verified install
# echo is bundled (ADR 0174): never from the lock, see bundled_echo below.
for m in snap ctrl nano; do
  line="$(installed_module "$m")"
  if [[ -z "$line" ]]; then
    info "" "$m: not installed under $data_dir"
    continue
  fi
  IFS=$'\t' read -r bin ver src dr cdh <<<"$line"
  why="$(not_installed_by_qrtz "$m" "$bin" "$src" "$dr" "$cdh")"
  if [[ -n "$why" ]]; then
    info "" "$m $ver: not used for gate (h): $why"
  else
    real_modules+=("$m"$'\t'"$bin"$'\t'"$ver")
    info "" "$m $ver: installed by qrtz ($src), team $release_team, notarized cdhash $cdh; entitlements: $(codesign -d --entitlements - --xml "$bin" 2>/dev/null | tr -d '\n' | grep -oE '<key>[^<]+</key><true/>' | sed -E 's/<key>([^<]+).*/\1/' | tr '\n' ' ')"
  fi
done
# Bundled echo (ADR 0174): qrtz.app/Contents/MacOS/qrtz-echo, a plain file (no symlink) in a signed qrtz.app, satisfying
# the requirement rebuilt from its name and team (never read from a lock). Empty = it is.
not_bundled_by_qrtz() { # $1 = path
  local macos
  macos="$(dirname "$1")"
  [[ -d "$qrtz_app" ]] || { echo "no qrtz.app at $qrtz_app (set QRTZ_APP)"; return; }
  [[ -L "$qrtz_app" || -L "$qrtz_app/Contents" || -L "$macos" || -L "$1" ]] && { echo "a symlink on the way to $1"; return; }
  [[ -f "$1" ]] || { echo "$1 is missing or not a plain file"; return; }
  codesign --verify --strict "$qrtz_app" >/dev/null 2>&1 || { echo "$qrtz_app doesn't verify"; return; }
  codesign --verify --strict "-R=$(module_requirement echo)" "$1" >/dev/null 2>&1 || { echo "$1 doesn't satisfy ai.qrtz.echo's release requirement"; return; }
  true
}
echo_bin="$qrtz_app/Contents/MacOS/qrtz-echo"
why="$(not_bundled_by_qrtz "$echo_bin")"
if [[ -n "$why" ]]; then
  info "" "echo (bundled): not used for gate (h): $why"
else
  real_modules+=("echo"$'\t'"$echo_bin"$'\t'"bundled")
  info "" "echo (bundled in $qrtz_app), team $release_team; entitlements: $(codesign -d --entitlements - --xml "$echo_bin" 2>/dev/null | tr -d '\n' | grep -oE '<key>[^<]+</key><true/>' | sed -E 's/<key>([^<]+).*/\1/' | tr '\n' ' ')"
fi

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
    fail "(h)" "stand-in: the host couldn't run the extension helper: $s"
    return
  fi
  c="$(cat "$out")"
  if [[ "$(field "$c" trusted)" == "true" ]]; then
    pass "(h)" "stand-in: extension helper ($ext_id, outside the host bundle) spawned by the host: AXIsProcessTrusted=true from qrtz's grant, no Privacy entry of its own ($signing)"
  else
    fail "(h)" "stand-in: extension helper spawned by the granted host is not trusted ($c): it's not attributed to qrtz, so ADR 0154's single-entry model needs a launcher change"
  fi
  # The real thing (ADR 0169): each module the installer put under the data dir, run from its package dir by the host
  # (`--qrtz-gate-probe ax` runs before anything else in every module executable), so it also shows the helper runs
  # from <data>/packages/.
  if ((${#real_modules[@]} == 0)); then
    skip "(h)" "no bundled echo in a signed $qrtz_app and no module installed through the marketplace under $data_dir (install snap or ctrl from the marketplace, or \`qrtz install snap\`, and rerun); a locally built helper doesn't count"
  fi
  local r id bin ver rc
  for r in ${real_modules[@]+"${real_modules[@]}"}; do
    IFS=$'\t' read -r id bin ver <<<"$r"
    tccutil reset Accessibility "ai.qrtz.$id" >/dev/null 2>&1
    rm -f "$out"
    s="$(probe host spawn "$bin" -- --qrtz-gate-probe ax --out "$out")"
    if [[ "$(field "$s" child_exit)" != "0" || ! -f "$out" ]]; then
      fail "(h)" "$id $ver (installed): the host couldn't run $bin: $s"
      continue
    fi
    rc="$(cat "$out")"
    if [[ "$(field "$rc" trusted)" == "true" ]]; then
      pass "(h)" "$id $ver, installed by qrtz's installer (or bundled in qrtz.app) and run by the host: AXIsProcessTrusted=true from qrtz's grant (ai.qrtz.$id has no Privacy entry of its own)"
    else
      fail "(h)" "$id $ver (installed) spawned by the granted host is not trusted ($rc): ADR 0154's single entry doesn't hold for real modules"
    fi
  done
  # Mic: the same attribution (INFO only, nothing prompts): the child's status should match the host's.
  rm -f "$out"
  probe host spawn "$ext_bin" -- mic --out "$out" >/dev/null
  info "(h)" "mic status: host=$(field "$(probe host mic)" status) extension helper=$(field "$(cat "$out" 2>/dev/null)" status) (should match)"
  # Contrast (not a gate): the same helper run from this shell is attributed to the terminal, not to qrtz.
  info "(h)" "run from this shell instead (attributed to the terminal): $("$ext_bin" ax --out /dev/null | tr -d '\n')"
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

# ---- (g) sideload freshness ---------------------------------------------------------------------------------------
gate_g() {
  skip "(g)" "sideload freshness is installer logic, not a Mac property: cargo test -p qrtz-publish --test gate_g runs ADR 0144's four cases on ADR 0157's installer (expired-list sideload installs marked stale, revoked id refused before the prompt, older snapshot refused, later fresh index disables a revoked module) plus the freshness exemption"
}

g_retired
gate_h
gate_d
gate_g
if ((fails)); then
  echo "$fails gate check(s) FAILED."
  echo "If (h) failed, extension helpers aren't attributed to qrtz: ADR 0154's single Privacy entry needs a launcher change."
  exit 1
fi
echo "no gate check failed (SKIPs say what is still missing)"
