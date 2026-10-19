# app-update test fixtures (TEST ONLY)

Throwaway minisign keys made with `tauri signer generate --ci` on the box for these tests only, by `make.sh`
(`TAURI=<node_modules/.bin/tauri> make.sh` regenerates every file below with new keys); their private keys were
deleted right after signing and never were the real app-update key (that one is made on the release Mac by
`scripts/ci/setup-secrets.sh app-update`, ADR 0181). Nothing here is trusted by a qrtz build.

- `test.pub`: the key `good-0.3.0.sig` and `old-0.2.5.sig` were made with.
- `other.pub`: a second key; `otherkey-0.3.0.sig` is from it.
- `artifact.tar.gz`: the signed bytes (not a real archive).
- `good-0.3.0.sig`: `tauri signer sign --app-version 0.3.0 artifact.tar.gz` (trusted comment `version:0.3.0`).
- `old-0.2.5.sig`: the same bytes signed for 0.2.5 (a feed pairing it with 0.3.0 must be refused).
- `feed-<name>.json`: signed `latest.json` envelopes (the feed's bytes signed by `test.pub`, expires
  2026-11-15T00:00:00Z): `0.3.0`, `0.2.5`, `0.2.0`, `0.1.9`, `0.3.0-otherkey-archive` (the archive signed by
  `other.pub`), `0.3.0-old-archive-sig` (the 0.2.5 archive signature), `0.3.0-expired` (expires 2026-10-01) and
  `0.3.0-otherkey-feed` (the feed itself signed by `other.pub`).
