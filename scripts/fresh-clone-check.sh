#!/usr/bin/env bash
# Proves the repo builds and passes from scratch: fresh clone of HEAD, npm ci, lint, typecheck, test, package, e2e.
set -euo pipefail
src="$(git rev-parse --show-toplevel)"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
git clone -q "${1:-$src}" "$dir/qrtz"
cd "$dir/qrtz"
npm ci
npm run lint
npx tsc -b
npm test
npm run package
if command -v xvfb-run >/dev/null && [ "$(uname)" = Linux ]; then xvfb-run -a npm run e2e; else npm run e2e; fi
echo "fresh-clone check passed ($(git rev-parse --short HEAD))"
