#!/usr/bin/env bash
# QRT-292: the module.json signing rules scripts/modules/build-packages.sh holds nano, snap and ctrl to: module_code
# (parse module.json, check its bundleId and that its designatedRequirement pins the identifier and the signing team)
# and check_package (the signed executable satisfies it). Sourced; no secret, no network. Bash 3.2 compatible.

# req_has_bypass <requirement>: true if the requirement uses or / || / not / ! anywhere outside a string literal, in
# any spacing or case. Such a clause could make the identifier or team pin optional (`identifier "x" and ... or
# anchor apple`), so a substring match wouldn't prove the pin is enforced. String literals are removed first and
# every operator character is split into its own token, so `"T"or`, `)or(`, `!cert`, `a||b` and `OR` are all caught.
# Fails closed (true) on what it can't tokenize: a backslash (escaped quote) or an unbalanced quote.
req_has_bypass() {
  local r="$1" q t toks
  [[ "$r" != *\\* ]] || return 0
  q="${r//[^\"]/}"
  (( ${#q} % 2 == 0 )) || return 0
  r="$(printf '%s' "$r" | sed -E 's/"[^"]*"/ _str_ /g; s/([][()!|&=<>,;])/ \1 /g' | tr '[:upper:]' '[:lower:]')"
  read -r -a toks <<<"$(printf '%s' "$r" | tr '\t\n\r\f\v' '     ')"
  for t in ${toks[@]+"${toks[@]}"}; do
    case "$t" in or | not | '!' | '|') return 0 ;; esac
  done
  return 1
}

# module_code <module.json> <team>: prints "<code.bundleId>\t<code.designatedRequirement>". Fails closed on: a
# module.json jq can't parse, a missing or malformed bundleId, a missing requirement, one with a tab or newline, one
# that doesn't start with `identifier "<bundleId>" and` or doesn't pin `certificate leaf[subject.OU] = "<team>"`, and
# one with an or/not clause (req_has_bypass).
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
  [[ "$req" != *$'\t'* && "$req" != *$'\n'* && "$req" != *$'\r'* ]] || { echo "module-code: $m: designatedRequirement has a tab or newline" >&2; return 1; }
  [[ "$req" == "identifier \"$id\" and "* ]] || { echo "module-code: $m: designatedRequirement does not start with identifier \"$id\"" >&2; return 1; }
  [[ "$req" == *"certificate leaf[subject.OU] = \"$team\""* ]] || { echo "module-code: $m: designatedRequirement does not pin signing team $team" >&2; return 1; }
  ! req_has_bypass "$req" || { echo "module-code: $m: designatedRequirement has an or/not clause that could bypass the identifier or team pin" >&2; return 1; }
  printf '%s\t%s\n' "$id" "$req"
}

# check_package <exe> <module.json> <identifier> <team> <identity>: the signed package executable passes module_code
# for its module.json, module.json's bundleId is <identifier> (build-packages' table), the signature's identifier is
# that, and the signature satisfies the requirement. With a "Developer ID Application:" identity that is module.json's
# full designatedRequirement (identifier + Developer ID + team, what the launcher checks); an ad-hoc or Apple
# Development signature can't meet that by construction, so it is held to the identifier clause and the package is
# dev-only (a release build refuses it at load). Prints the reason and returns nonzero. macOS only (codesign).
check_package() {
  local exe="$1" m="$2" want="$3" team="$4" identity="$5" code id req got
  code="$(module_code "$m" "$team" 2>&1)" || { echo "$code"; return 1; }
  IFS=$'\t' read -r id req <<<"$code"
  [[ "$id" == "$want" ]] || { echo "$m: code.bundleId is '$id', build-packages signs as '$want'"; return 1; }
  [[ "$identity" == "Developer ID Application:"* ]] || req="identifier \"$id\""
  got="$(codesign -dv "$exe" 2>&1 | sed -n 's/^Identifier=//p')"
  [[ "$got" == "$id" ]] || { echo "signing identifier is '$got', want '$id' (module.json code.bundleId)"; return 1; }
  codesign --verify --strict -R="$req" "$exe" 2>/dev/null || { echo "does not satisfy the launch requirement from module.json ($id)"; return 1; }
}
