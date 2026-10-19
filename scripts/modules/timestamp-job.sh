#!/usr/bin/env bash
# The automatic timestamp re-sign (ADR 0153 §3, ADR 0162): a launchd job on the release Mac that runs
# `publish.sh --timestamp` every day at 09:00 and again at every login/wake after a missed run. The timestamp is
# signed for 7 days and clients refuse it after 8, so a daily run leaves a week of slack for a Mac that's asleep,
# offline or travelling; a failure posts a macOS notification and is logged to ~/Library/Logs/qrtz-timestamp.log.
#
#   scripts/modules/timestamp-job.sh install     write and load ~/Library/LaunchAgents/ai.qrtz.timestamp.plist
#   scripts/modules/timestamp-job.sh run         what the job runs (credentials from the login keychain)
#   scripts/modules/timestamp-job.sh status | uninstall
#
# Keys it uses: only ~/.qrtz-keys/timestamp.key (it never reads the index key or a root key). Credentials come from
# the login keychain at run time (service qrtz-r2, account = the access key id), never from a file.
# Needs, once: `security add-generic-password -s qrtz-r2 -a <R2 access key id> -w`;
# R2_ACCOUNT_ID in ~/.qrtz-keys/r2-account-id (not secret). ADR 0167: R2 is the sole package host (no B2 key).
set -euo pipefail
label=ai.qrtz.timestamp
plist="$HOME/Library/LaunchAgents/$label.plist"
repo="$(cd "$(dirname "$0")/../.." && pwd)"
self="$repo/scripts/modules/timestamp-job.sh"
log="$HOME/Library/Logs/qrtz-timestamp.log"
keys="${QRTZ_KEYS:-$HOME/.qrtz-keys}"

account_of() { # <service>: the account (access key id) of a keychain item
  security find-generic-password -s "$1" 2>/dev/null | sed -n 's/^ *"acct"<blob>="\(.*\)"$/\1/p'
}

case "${1:-}" in
  install)
    bin="${CARGO_TARGET_DIR:-$repo/target}/release/qrtz-publish"
    (cd "$repo" && cargo build --release -p qrtz-publish)
    [[ -f "$keys/r2-account-id" ]] || {
      echo "write your Cloudflare account id to $keys/r2-account-id first" >&2
      exit 1
    }
    mkdir -p "$(dirname "$plist")" "$(dirname "$log")"
    cat >"$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key><array><string>/bin/bash</string><string>$self</string><string>run</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>QRTZ_PUBLISH_BIN</key><string>$bin</string>
    <key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>StartCalendarInterval</key><dict><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>$log</string>
  <key>StandardErrorPath</key><string>$log</string>
</dict></plist>
PLIST
    launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$plist"
    echo "installed $plist (daily 09:00, and at load); log: $log"
    ;;
  run)
    echo "== $(date -u +%FT%TZ)"
    export R2_ACCOUNT_ID R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY
    R2_ACCOUNT_ID="$(tr -d '[:space:]' <"$keys/r2-account-id")"
    R2_ACCESS_KEY_ID="$(account_of qrtz-r2)"
    R2_SECRET_ACCESS_KEY="$(security find-generic-password -s qrtz-r2 -w)"
    if ! "$repo/scripts/modules/publish.sh" --timestamp; then
      osascript -e 'display notification "The weekly re-sign failed; see ~/Library/Logs/qrtz-timestamp.log. Installs stop 8 days after the last good one." with title "qrtz package index"' || true
      exit 1
    fi
    ;;
  status)
    launchctl print "gui/$(id -u)/$label" 2>/dev/null | grep -E "state|last exit" || echo "not loaded"
    tail -5 "$log" 2>/dev/null || true
    ;;
  uninstall)
    launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    rm -f "$plist"
    echo "removed $plist"
    ;;
  *)
    sed -n '2,16p' "$0"
    exit 2
    ;;
esac
