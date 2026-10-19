# shellcheck shell=bash
# ADR 0185 (P3.4): which entitlements a plist grants, by the same rule as the app's runtime parser
# (crates/qrtz-app/src/echo_updates.rs granted_entitlements): whitespace dropped, then every <key> whose value isn't
# <false/>, nested keys included, sorted bytewise. Both are tested on spec/vectors/echo-entitlements/.
# Source it; mac-gate.sh check e6 uses it.

# stdin: an entitlements plist (XML). stdout: the granted keys, one per line.
granted_entitlements() {
  tr -d ' \t\n\r' | grep -oE '<key>[^<]*</key><[^>]*>' | grep -vE '<false/>$' |
    sed -E 's#^<key>([^<]*)</key>.*#\1#' | LC_ALL=C sort || true
}

# stdin: an entitlements plist (XML). Exit 0 if it grants exactly the microphone (echo's only entitlement).
entitlements_are_mic_only() {
  [[ "$(granted_entitlements)" == "com.apple.security.device.audio-input" ]]
}
