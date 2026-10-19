#!/usr/bin/env bash
# ADR 0220 (amendment L2): the notes vault's Mac gate. Runs the vault tests on macOS, where the paths that differ from
# Linux are real: renamex_np(RENAME_EXCL) for no-replace renames, /private/var/folders temp dirs and the /private,
# /Users and ~/.* folder refusals. Builds and runs tests only; launches no app and touches no installed qrtz.
# Usage: scripts/nano/mac-vault-gate.sh   (uses $CARGO_TARGET_DIR if set, else the repo's target/)
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "$(uname -s)" != "Darwin" ]; then
  echo "SKIP not macOS ($(uname -s)); these tests also run on Linux, but the gate is the Mac run"
  exit 0
fi
cargo test -p qrtz-storage --test vault
cargo test -p qrtz-app --test e2e_vault
echo "PASS nano vault Mac gate"
