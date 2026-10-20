#!/usr/bin/env bash
# QRT-255: the one source of truth for the helpers bundled into qrtz.app/Contents/MacOS is each module's own
# modules/<id>/module.json (launch.kind "bundled-helper"): launch.entry is the file name, code.bundleId the signing
# identifier and code.designatedRequirement the requirement the launcher (crates/qrtz-app/src/installed.rs
# launch_requirement) checks it against. 0.2.4 shipped qrtz-echo signed as "qrtz-echo" (the file name) while the
# launcher wants ai.qrtz.echo, so the app refused it. sign-mac.sh signs and verify-bundle.sh checks from this table,
# so a helper can't be signed as one thing and launched as another, and a new helper needs no script edit.
# Sourced by sign-mac.sh and verify-bundle.sh; no secret, no network.

# bundled_helpers <modules dir>: one line per bundled helper, tab-separated:
#   entry  identifier  entitlements(repo path | none)  designatedRequirement
# Fails closed (nonzero, message on stderr) on anything it can't vouch for: a missing identifier or requirement, a
# requirement that doesn't name the identifier, an entry with a path in it, Agent Dash's network helper (ADR 0231:
# never bundled), or an entitlements file whose keys differ from module.json's entitlements list.
bundled_helpers() {
  local dir="${1:?bundled_helpers <modules dir>}" repo m entry id req ents want got f n=0 out=""
  repo="$(cd "$dir/.." && pwd)"
  command -v jq >/dev/null 2>&1 || { echo "bundled-helpers: jq is required to read module.json" >&2; return 1; }
  for m in "$dir"/*/module.json; do
    [[ -f "$m" ]] || continue
    [[ "$(jq -r '.launch.kind // ""' "$m")" == bundled-helper ]] || continue
    entry="$(jq -r '.launch.entry // ""' "$m")"
    id="$(jq -r '.code.bundleId // ""' "$m")"
    req="$(jq -r '.code.designatedRequirement // ""' "$m")"
    [[ "$entry" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "bundled-helpers: $m: launch.entry '$entry' must be a plain file name" >&2; return 1; }
    case "$entry$id" in *dash-net* | *dash_net*) echo "bundled-helpers: $m: Agent Dash's network helper is never bundled (ADR 0231)" >&2; return 1 ;; esac
    [[ "$id" =~ ^[A-Za-z0-9.-]+$ ]] || { echo "bundled-helpers: $m: code.bundleId missing or malformed" >&2; return 1; }
    [[ -n "$req" ]] || { echo "bundled-helpers: $m: code.designatedRequirement missing" >&2; return 1; }
    [[ "$req" == *"identifier \"$id\""* ]] || { echo "bundled-helpers: $m: designatedRequirement does not name identifier \"$id\"" >&2; return 1; }
    [[ "$req" != *$'\t'* && "$req" != *$'\n'* ]] || { echo "bundled-helpers: $m: designatedRequirement has a tab or newline" >&2; return 1; }
    want="$(jq -r '(.entitlements // [])[]' "$m" | sort | tr '\n' ' ')"
    if [[ -z "$want" ]]; then
      ents=none
    else
      ents="native/qrtz-helper/entitlements/$id.entitlements"
      f="$repo/$ents"
      [[ -f "$f" ]] || { echo "bundled-helpers: $m: entitlements file $ents missing" >&2; return 1; }
      got="$(grep -o '<key>[^<]*</key>' "$f" | sed 's/<[^>]*>//g' | sort | tr '\n' ' ')"
      [[ "$got" == "$want" ]] || { echo "bundled-helpers: $ents has [$got], module.json lists [$want]" >&2; return 1; }
    fi
    out+="$entry"$'\t'"$id"$'\t'"$ents"$'\t'"$req"$'\n'
    n=$((n + 1))
  done
  ((n)) || { echo "bundled-helpers: no bundled helper in $dir/*/module.json" >&2; return 1; }
  printf '%s' "$out"
}

# check_helper <binary> <identifier> <requirement>: the signature names exactly <identifier> and satisfies
# <requirement> (codesign --verify --strict -R). Prints the reason and returns nonzero otherwise.
check_helper() {
  local bin="$1" id="$2" req="$3" info got
  [[ -f "$bin" ]] || { echo "missing"; return 1; }
  info="$(codesign -dv "$bin" 2>&1)" || { echo "unsigned or unreadable signature"; return 1; }
  got="$(sed -n 's/^Identifier=//p' <<<"$info")"
  [[ "$got" == "$id" ]] || { echo "signing identifier is '$got', want '$id' (module.json code.bundleId)"; return 1; }
  codesign --verify --strict -R="$req" "$bin" 2>/dev/null || { echo "does not satisfy the launch requirement from module.json ($id)"; return 1; }
}
