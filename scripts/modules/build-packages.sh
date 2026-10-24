#!/usr/bin/env bash
# ADR 0169: build each module's own package (qrtz ships empty; a module's code lives only in its package), sign its
# executable with the module's identifier (the one its lock entry and module.json pin; confinement-map.json
# moduleCode, checked by crates/qrtz-lifecycle/tests/offline_graph.rs), and optionally make a local dev index.
#
#   scripts/modules/build-packages.sh [--out <dir>] [--dev-index] [<module>...]   (default: nano ctrl snap; echo is bundled, ADR 0174)
#
# Output: <out>/packages/<module>/{qrtz.json, bin/qrtz-<module>} (default out: ~/qrtz-packages), ready for
# QRTZ_PUBLISH_PACKAGES in scripts/modules/publish.sh, which notarizes each before it enters the index (ADR 0170).
#
# --dev-index: also a signed index in <out>/index with throwaway keys made in <out>/dev-keys (never the release keys,
# never in the repo), holding the packages and the pinned models (ADR 0174: no inference.voice in v1) found under
# $QRTZ_MODELS_DIR (only hashed against their pins: like the real index, the dev index lists each model file's pinned
# Hugging Face URL and the app downloads it from there, ADR 0171). Its notarization records are FAKE (submission 00000000-…): only a debug build reads a dev index
# (QRTZ_DEV_INDEX_DIR + QRTZ_DEV_ROOT1_SHA256, printed at the end), and it checks signatures with DevCodeChecks.
# A release build refuses both the index and the records.
#
# Env: QRTZ_SIGN_ID (default: the first "Developer ID Application" identity, else "Apple Development", else ad-hoc);
#      QRTZ_TEAM_ID (MR26CCZ496); QRTZ_MODELS_DIR (~/Library/Application Support/ai.qrtz.desktop/models);
#      SWIFT_BUILD_FLAGS (extra flags for swift build, e.g. --disable-keychain).
set -euo pipefail

die() { echo "build-packages: $*" >&2; exit 1; }
# QRT-292: module_code / check_package, the module.json identifier and team-pin rules.
# shellcheck source=./module-code.sh
source "$(cd "$(dirname "$0")" && pwd)/module-code.sh"
[[ "$(uname -s)" == Darwin ]] || die "macOS only (the module executables are Swift and AppKit)"
repo="$(cd "$(dirname "$0")/../.." && pwd)"
out="$HOME/qrtz-packages"
dev_index=0
want=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) out="${2:?--out <dir>}"; shift 2 ;;
    --dev-index) dev_index=1; shift ;;
    -h | --help) sed -n '2,20p' "$0"; exit 0 ;;
    -*) die "unknown option $1" ;;
    *) want+=("$1"); shift ;;
  esac
done

# module identifier entitlements. Must match docs/modules/confinement-map.json moduleCode (offline_graph.rs).
MODULES="
nano ai.qrtz.nano native/qrtz-helper/entitlements/ai.qrtz.nano.entitlements
ctrl ai.qrtz.ctrl native/qrtz-helper/entitlements/ai.qrtz.ctrl.entitlements
snap ai.qrtz.snap native/qrtz-helper/entitlements/ai.qrtz.snap.entitlements
"
[[ ${#want[@]} -gt 0 ]] || want=(nano ctrl snap)
# ADR 0174: echo ships inside qrtz.app (scripts/build-helper.mjs, sign-mac.sh), never as a package.

identity="${QRTZ_SIGN_ID:-}"
if [[ -z "$identity" ]]; then
  ids="$(security find-identity -v -p codesigning 2>/dev/null || true)"
  identity="$(sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' <<<"$ids" | head -1)"
  [[ -n "$identity" ]] || identity="$(sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' <<<"$ids" | head -1)"
  [[ -n "$identity" ]] || identity="-"
fi
team="${QRTZ_TEAM_ID:-MR26CCZ496}"
# Refuse before building: every requested module.json must parse and pin its identifier and the signing team.
for m in "${want[@]}"; do
  module_code "$repo/modules/$m/module.json" "$team" >/dev/null || die "$m: modules/$m/module.json fails the signing rules"
done
ts=(--timestamp=none)
[[ "$identity" == "Developer ID Application:"* ]] && ts=(--timestamp)

echo "== swift build (release, arm64)"
helper="$repo/native/qrtz-helper"
# shellcheck disable=SC2086
(cd "$helper" && for p in "${want[@]}"; do swift build -c release --arch arm64 ${SWIFT_BUILD_FLAGS:-} --product "qrtz-$p"; done)
bin_dir="$(cd "$helper" && swift build -c release --arch arm64 ${SWIFT_BUILD_FLAGS:-} --show-bin-path)"

echo "== stage and sign ($identity)"
for m in "${want[@]}"; do
  line="$(awk -v m="$m" '$1 == m' <<<"$MODULES")"
  [[ -n "$line" ]] || die "$m: not a module (${MODULES//$'\n'/ })"
  read -r _ id ent <<<"$line"
  pkg="$out/packages/$m"
  rm -rf "$pkg"
  mkdir -p "$pkg/bin"
  cp "$repo/modules/$m/qrtz.json" "$pkg/qrtz.json"
  exe="$pkg/bin/qrtz-$m"
  cp "$bin_dir/qrtz-$m" "$exe"
  chmod 755 "$exe"
  codesign --force --options runtime "${ts[@]}" --identifier "$id" --entitlements "$repo/$ent" --sign "$identity" "$exe"
  info="$(codesign -dv "$exe" 2>&1)"
  grep -qx "Identifier=$id" <<<"$info" || die "$exe: not signed as $id"
  # QRT-292: codesign --verify --strict -R=<modules/$m/module.json designatedRequirement>; no override.
  why="$(check_package "$exe" "$repo/modules/$m/module.json" "$id" "$team" "$identity")" || die "$m: $why"
  [[ "$identity" == "Developer ID Application:"* ]] || echo "  $m: not Developer ID: checked the identifier clause only (dev-only package)" >&2
  echo "  $m: $exe ($id)"
done

[[ "$dev_index" == 1 ]] || { echo "built in $out/packages (publish: QRTZ_PUBLISH_PACKAGES=\"$out/packages/*\")"; exit 0; }

echo "== dev index (throwaway keys; fake notarization; debug builds only)"
models_dir="${QRTZ_MODELS_DIR:-$HOME/Library/Application Support/ai.qrtz.desktop/models}"
(cd "$repo" && cargo build --release -p qrtz-publish)
pub="${CARGO_TARGET_DIR:-$repo/target}/release/qrtz-publish"
keys="$out/dev-keys"
idx="$out/index"
if [[ ! -f "$keys/index.key" ]]; then
  rm -rf "$keys" "$idx"
  for r in root-a timestamp index; do "$pub" keygen --dir "$keys" --role "$r" >/dev/null; done
fi
if [[ ! -f "$idx/v1/root/1.root.json" ]]; then
  "$pub" root --out "$idx" --keys "$keys" >/dev/null
fi
pin="$(shasum -a 256 "$idx/v1/root/1.root.json" | cut -d' ' -f1)"

fake() { # file: a FAKE notarization record for a dev index (a release build refuses it: the cdhash isn't real)
  local sha
  sha="$(shasum -a 256 "$1" | cut -d' ' -f1)"
  printf '{ "submissionId": "00000000-0000-4000-8000-000000000000", "status": "Accepted", "codeSha256": "%s", "cdhash": "%s" }' "$sha" "${sha:0:40}"
}
model_dir() {
  case "$1" in
    model.parakeet-tdt-v2-en) echo parakeet-tdt-v2 ;;
    model.nemotron) echo nemotron-560ms ;;
    model.nemotron-diarizer) echo nemotron3-diarizer ;;
    model.localvqe) echo localvqe ;;
    model.laya) echo laya ;;
  esac
}
spec="$out/dev-spec.json"
{
  printf '{\n  "teamId": "%s",\n  "pinnedModels": ["%s", "%s"],\n  "models": [' "$team" \
    "$repo/crates/qrtz-app/src/local_models.json" "$repo/crates/qrtz-app/src/ctrl_laya_model.json"
  sep=""
  for n in model.parakeet-tdt-v2-en model.nemotron model.nemotron-diarizer model.localvqe model.laya; do
    d="$models_dir/$(model_dir "$n")"
    [[ -d "$d" ]] || { echo "  (no $n under $models_dir: a module needing it won't install)" >&2; continue; }
    printf '%s\n    { "name": "%s", "dir": "%s" }' "$sep" "$n" "$d"
    sep=","
  done
  printf '\n  ],\n  "packages": ['
  sep=""
  for m in "${want[@]}"; do
    printf '%s\n    { "dir": "%s", "notarization": %s }' "$sep" "$out/packages/$m" "$(fake "$out/packages/$m/bin/qrtz-$m")"
    sep=","
  done
  printf '\n  ]\n}\n'
} >"$spec"
# --dev-index: these records are FAKE (above); a plain `build` checks every record against its code's cdhash.
"$pub" build --out "$idx" --spec "$spec" --keys "$keys" --dev-index
"$pub" verify --out "$idx" --root-sha256 "$pin" >/dev/null
echo
echo "dev index ready. Run a debug qrtz with:"
echo "  export QRTZ_DEV_INDEX_DIR=\"$idx\" QRTZ_DEV_ROOT1_SHA256=$pin"
