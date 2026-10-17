#!/usr/bin/env bash
# Proves the repo builds and passes from scratch: fresh clone of HEAD, bun install (frozen), lint, typecheck,
# vitest, the renderer build and the Rust workspace tests.
set -euo pipefail
src="$(git rev-parse --show-toplevel)"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
git clone -q "${1:-$src}" "$dir/qrtz"
cd "$dir/qrtz"
bun install --frozen-lockfile
bun run lint
bun run typecheck
bun run test
bun run --cwd apps/desktop build
cargo test --workspace
echo "fresh-clone check passed ($(git rev-parse --short HEAD))"
