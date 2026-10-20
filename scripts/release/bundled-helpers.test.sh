#!/usr/bin/env bash
# QRT-255 negative test (macOS, no secret, no identity): a bundled helper signed with the wrong identifier, as 0.2.4's
# qrtz-echo was (file name instead of ai.qrtz.echo), must fail the helper check that sign-mac.sh and verify-bundle.sh
# use, and so fail the release. Ad-hoc signatures can't meet the real Developer ID requirement, so this uses a
# test-only modules dir whose module.json pins `identifier "ai.qrtz.qrt255test"`; the real gate always reads the repo's
# modules/*/module.json and has no override. Run in CI: release-unsigned job.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
source "$here/bundled-helpers.sh"
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
pass=0; bad=0
expect_fail() { if why="$("$@")"; then echo "FAIL: $* passed, should have failed" >&2; bad=$((bad + 1)); else echo "ok: refused ($why)"; pass=$((pass + 1)); fi; }
expect_pass() { if why="$("$@")"; then echo "ok: accepted $3"; pass=$((pass + 1)); else echo "FAIL: $* refused: $why" >&2; bad=$((bad + 1)); fi; }

# The real module.json table resolves echo to ai.qrtz.echo with its own entitlements and requirement.
real="$(bundled_helpers "$here/../../modules")"
grep -q $'^qrtz-echo\tai.qrtz.echo\tnative/qrtz-helper/entitlements/ai.qrtz.echo.entitlements\tidentifier "ai.qrtz.echo" and anchor apple generic' <<<"$real" \
  || { echo "FAIL: modules/echo/module.json did not resolve to ai.qrtz.echo: $real" >&2; bad=$((bad + 1)); }

mkdir -p "$t/modules/testhelper"
cat >"$t/modules/testhelper/module.json" <<'JSON'
{"id":"testhelper","code":{"bundleId":"ai.qrtz.qrt255test","designatedRequirement":"identifier \"ai.qrtz.qrt255test\""},
 "launch":{"kind":"bundled-helper","entry":"qrtz-testhelper"},"entitlements":[]}
JSON
row="$(bundled_helpers "$t/modules")"
IFS=$'\t' read -r entry id ent req <<<"$row"
[[ "$entry/$id/$ent/$req" == 'qrtz-testhelper/ai.qrtz.qrt255test/none/identifier "ai.qrtz.qrt255test"' ]] \
  || { echo "FAIL: test module.json parsed as $row" >&2; bad=$((bad + 1)); }

bin="$t/$entry"
cp /usr/bin/true "$bin"
codesign --force --identifier "$entry" --sign - "$bin"          # the 0.2.4 bug: identifier = file name
expect_fail check_helper "$bin" "$id" "$req"
codesign --force --identifier ai.qrtz.other --sign - "$bin"     # any other identifier
expect_fail check_helper "$bin" "$id" "$req"
codesign --force --identifier "$id" --sign - "$bin"             # right identifier
expect_pass check_helper "$bin" "$id" "$req"
expect_fail check_helper "$bin" "$id" "identifier \"$id\" and anchor apple generic" # but not Developer ID
expect_fail check_helper "$t/missing" "$id" "$req"

# module.json itself is checked: a requirement naming another identifier, or no requirement, is refused.
for bad_json in '{"code":{"bundleId":"ai.qrtz.a","designatedRequirement":"identifier \"ai.qrtz.b\""},"launch":{"kind":"bundled-helper","entry":"x"}}' \
                '{"code":{"bundleId":"ai.qrtz.a"},"launch":{"kind":"bundled-helper","entry":"x"}}' \
                '{"code":{"bundleId":"ai.qrtz.a","designatedRequirement":"identifier \"ai.qrtz.a\""},"launch":{"kind":"bundled-helper","entry":"qrtz-dash-net"}}'; do
  echo "$bad_json" >"$t/modules/testhelper/module.json"
  expect_fail bundled_helpers "$t/modules"
done
((bad == 0)) || { echo "bundled-helpers test: $bad failed" >&2; exit 1; }
echo "bundled-helpers test: all $pass passed"
