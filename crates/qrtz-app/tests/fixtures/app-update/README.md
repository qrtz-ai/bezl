# app-update test fixtures (TEST ONLY)

Throwaway minisign keys made with `tauri signer generate --ci` on the box for these tests only; their private keys
were deleted right after signing and never were the real app-update key (that one is made on the release Mac by
`scripts/ci/setup-secrets.sh app-update`, ADR 0181). Nothing here is trusted by a qrtz build.

- `test.pub`: the key `good-0.3.0.sig` and `old-0.2.5.sig` were made with.
- `other.pub`: a second key; `otherkey-0.3.0.sig` is from it.
- `artifact.tar.gz`: the signed bytes (not a real archive).
- `good-0.3.0.sig`: `tauri signer sign --app-version 0.3.0 artifact.tar.gz` (trusted comment `version:0.3.0`).
- `old-0.2.5.sig`: the same bytes signed for 0.2.5 (a feed pairing it with 0.3.0 must be refused).
