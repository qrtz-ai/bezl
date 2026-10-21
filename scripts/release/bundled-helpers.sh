#!/usr/bin/env bash
# QRT-255: the one source of truth for the helpers bundled into qrtz.app/Contents/MacOS is each module's own
# modules/<id>/module.json (launch.kind "bundled-helper"): launch.entry is the file name, code.bundleId the signing
# identifier and code.designatedRequirement the requirement the launcher (crates/qrtz-app/src/installed.rs
# launch_requirement) checks it against. 0.2.4 shipped qrtz-echo signed as "qrtz-echo" (the file name) while the
# launcher wants ai.qrtz.echo, so the app refused it. sign-mac.sh signs and verify-bundle.sh checks from this table,
# so a helper can't be signed as one thing and launched as another, and a new helper needs no script edit.
# Sourced by sign-mac.sh and verify-bundle.sh; no secret, no network.

# req_has_bypass <requirement>: true if the requirement has an or / || / not / ! anywhere outside a string literal or
# comment, in any spacing or case. It tokenizes rather than pattern-matching: a word is a run of identifier
# characters (A-Z a-z 0-9 _ . -), so every other character, a `"` included, ends one, and `"T"or`, `)or(`, `]not`,
# `*/or`, `a||b` and `OR` are all caught. Fails closed (true) on what it can't tokenize safely: a backslash, a
# non-printable or non-ASCII character, an unterminated string or comment, and any comment other than csreq's own
# `/* exists */` (a comment could otherwise hide the team pin the substring check looks for). Bash 3.2 compatible.
req_has_bypass() {
  local LC_ALL=C
  local r="$1" i=0 n c word="" note="" state=code
  [[ "$r" != *\\* && "$r" != *[^[:print:]]* ]] || return 0
  n=${#r}
  while ((i <= n)); do
    c="${r:i:1}"
    case "$state" in
      string) [[ "$c" != '"' ]] || state=code; ((i < n)) || return 0 ;;
      comment)
        if [[ "$c" == '*' && "${r:i+1:1}" == / ]]; then
          [[ "$note" == " exists " ]] || return 0
          state=code; note=""; i=$((i + 1))
        else
          note+="$c"
        fi
        ((i < n)) || return 0 ;;
      code)
        if [[ -n "$c" && "$c" == [[:alnum:]_.-] ]]; then
          word+="$c"
        else
          case "$(printf '%s' "$word" | tr '[:upper:]' '[:lower:]')" in or | not) return 0 ;; esac
          word=""
          case "$c" in
            '!' | '|') return 0 ;;
            '"') state=string ;;
            /) if [[ "${r:i+1:1}" == '*' ]]; then state=comment; i=$((i + 1)); fi ;;
          esac
        fi ;;
    esac
    i=$((i + 1))
  done
  return 1
}

# req_compiles_as_written <requirement>: on a Mac (csreq present; on Darwin csreq is required), compile the
# requirement and decompile it; fail unless the decompiled form has no or/not clause and equals the requirement as
# written, up to the quotes csreq drops around plain strings and whitespace. This catches anything macOS's parser reads
# differently from the tokenizer above. Without csreq (Linux CI) it returns 0: req_has_bypass still applies.
req_compiles_as_written() {
  local req="$1" d bin got norm_req norm_got
  if ! command -v csreq >/dev/null 2>&1; then
    [[ "$(uname -s)" != Darwin ]] || { echo "module-code: csreq is missing on macOS" >&2; return 1; }
    return 0
  fi
  d="$(mktemp -d)" || return 1
  bin="$d/req.bin"
  if ! csreq -r "=$req" -b "$bin" >/dev/null 2>&1 || ! got="$(csreq -r "$bin" -t 2>/dev/null)"; then
    rm -rf "$d"; echo "module-code: csreq can't compile the requirement" >&2; return 1
  fi
  rm -rf "$d"
  norm_req="$(printf '%s' "$req" | tr -d '"' | tr -s ' \t\n' '   ' | sed 's/^ //; s/ $//')"
  norm_got="$(printf '%s' "$got" | tr -d '"' | tr -s ' \t\n' '   ' | sed 's/^ //; s/ $//')"
  ! req_has_bypass "$got" || { echo "module-code: csreq reads the requirement with an or/not clause: $got" >&2; return 1; }
  [[ "$norm_got" == "$norm_req" ]] || { echo "module-code: csreq reads the requirement as: $got" >&2; return 1; }
}

# module_code <module.json> <team>: prints "<code.bundleId>\t<code.designatedRequirement>" (QRT-292: shared by
# bundled_helpers and scripts/modules/build-packages.sh, so packaged modules and bundled helpers obey one rule set).
# Fails closed on: a module.json jq can't parse, a missing or malformed bundleId, a missing requirement, one that
# doesn't name `identifier "<bundleId>"` or doesn't pin `certificate leaf[subject.OU] = "<team>"`, one with a tab or
# newline, and one using `or`/`||`/`not`/`!`: those clauses could make the identifier or team pin optional
# (`identifier "x" and ... or anchor apple`), so a substring match wouldn't prove the pin is enforced. Where macOS's
# csreq exists it also compiles the requirement and refuses it unless the compiled form decompiles back to the same
# requirement (req_compiles_as_written), so what the launcher would enforce is what was checked.
module_code() {
  local m="${1:?module_code <module.json> <team>}" team="${2:-}" id req
  [[ "$team" =~ ^[0-9A-Z]{10}$ ]] || { echo "module-code: signing team '$team' must be 10 characters, A-Z and 0-9" >&2; return 1; }
  command -v jq >/dev/null 2>&1 || { echo "module-code: jq is required to read module.json" >&2; return 1; }
  [[ -f "$m" ]] || { echo "module-code: $m: missing" >&2; return 1; }
  jq -e 'type == "object"' "$m" >/dev/null 2>&1 || { echo "module-code: $m: not valid JSON (jq can't parse it)" >&2; return 1; }
  id="$(jq -r '.code.bundleId // ""' "$m")" || { echo "module-code: $m: can't read code.bundleId" >&2; return 1; }
  req="$(jq -r '.code.designatedRequirement // ""' "$m")" || { echo "module-code: $m: can't read code.designatedRequirement" >&2; return 1; }
  [[ "$id" =~ ^[A-Za-z0-9.-]+$ ]] || { echo "module-code: $m: code.bundleId missing or malformed" >&2; return 1; }
  [[ -n "$req" ]] || { echo "module-code: $m: code.designatedRequirement missing" >&2; return 1; }
  [[ "$req" != *$'\t'* && "$req" != *$'\n'* ]] || { echo "module-code: $m: designatedRequirement has a tab or newline" >&2; return 1; }
  [[ "$req" == "identifier \"$id\" and "* ]] || { echo "module-code: $m: designatedRequirement does not start with identifier \"$id\"" >&2; return 1; }
  [[ "$req" == *"certificate leaf[subject.OU] = \"$team\""* ]] || { echo "module-code: $m: designatedRequirement does not pin signing team $team" >&2; return 1; }
  ! req_has_bypass "$req" || { echo "module-code: $m: designatedRequirement has an or/not clause that could bypass the identifier or team pin" >&2; return 1; }
  req_compiles_as_written "$req" || { echo "module-code: $m: designatedRequirement does not compile (csreq) to the requirement as written" >&2; return 1; }
  printf '%s\t%s\n' "$id" "$req"
}

# bundled_helpers <modules dir> <team>: one line per bundled helper, tab-separated:
#   entry  identifier  entitlements(repo path | none)  designatedRequirement
# Fails closed (nonzero, message on stderr) on anything it can't vouch for: a missing identifier or requirement, a
# module.json jq can't parse, a requirement that doesn't name the identifier or doesn't pin the signing team <team>, an entry with a path in it, Agent Dash's network helper (ADR 0231:
# never bundled), or an entitlements file whose keys differ from module.json's entitlements list.
bundled_helpers() {
  local dir="${1:?bundled_helpers <modules dir> <team>}" team="${2:-}" repo m kind entry id req code ents want got f n=0 out=""
  [[ "$team" =~ ^[0-9A-Z]{10}$ ]] || { echo "bundled-helpers: signing team '$team' must be 10 characters, A-Z and 0-9" >&2; return 1; }
  repo="$(cd "$dir/.." && pwd)"
  command -v jq >/dev/null 2>&1 || { echo "bundled-helpers: jq is required to read module.json" >&2; return 1; }
  for m in "$dir"/*/module.json; do
    [[ -f "$m" ]] || continue
    # An unparseable module.json fails: skipping it would silently drop its helper from signing and verification.
    jq -e 'type == "object"' "$m" >/dev/null 2>&1 || { echo "bundled-helpers: $m: not valid JSON (jq can't parse it)" >&2; return 1; }
    kind="$(jq -r '.launch.kind // ""' "$m")" || { echo "bundled-helpers: $m: can't read launch.kind" >&2; return 1; }
    [[ "$kind" == bundled-helper ]] || continue
    entry="$(jq -r '.launch.entry // ""' "$m")" || { echo "bundled-helpers: $m: can't read launch.entry" >&2; return 1; }
    id="$(jq -r '.code.bundleId // ""' "$m")" || { echo "bundled-helpers: $m: can't read code.bundleId" >&2; return 1; }
    [[ "$entry" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "bundled-helpers: $m: launch.entry '$entry' must be a plain file name" >&2; return 1; }
    case "$entry$id" in *dash-net* | *dash_net*) echo "bundled-helpers: $m: Agent Dash's network helper is never bundled (ADR 0231)" >&2; return 1 ;; esac
    code="$(module_code "$m" "$team")" || return 1
    IFS=$'\t' read -r id req <<<"$code"
    want="$(jq -r '(.entitlements // [])[]' "$m")" || { echo "bundled-helpers: $m: can't read entitlements" >&2; return 1; }
    want="$(sort <<<"$want" | sed '/^$/d' | tr '\n' ' ')"
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

# check_package <exe> <module.json> <identifier> <team> <identity> (QRT-292, scripts/modules/build-packages.sh): the
# signed package executable passes module_code's rules for its module.json, module.json's bundleId is <identifier>
# (build-packages' table), and the signature satisfies the requirement. With a "Developer ID Application:" identity
# that is module.json's full designatedRequirement (identifier + Developer ID + team, what the launcher checks);
# an ad-hoc or Apple Development signature can't meet that by construction, so it is held to the identifier
# clause and the package is dev-only (a release build refuses it at load). Prints the reason and returns nonzero.
check_package() {
  local exe="$1" m="$2" want="$3" team="$4" identity="$5" code id req
  code="$(module_code "$m" "$team" 2>&1)" || { echo "$code"; return 1; }
  IFS=$'\t' read -r id req <<<"$code"
  [[ "$id" == "$want" ]] || { echo "$m: code.bundleId is '$id', build-packages signs as '$want'"; return 1; }
  [[ "$identity" == "Developer ID Application:"* ]] || req="identifier \"$id\""
  check_helper "$exe" "$id" "$req"
}
