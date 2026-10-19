# Proposal: echo updates on its own

Status: proposal for Bozniak's review. Not accepted. Branch `proposal/echo-independent-updates` only; ADR 0174
stands on main: the qrtz-echo binary changes only when the whole signed qrtz.app is replaced. See ADR 0181,
"Proposal (not accepted)".

## What changes

echo ships a second way, as its own Developer ID-signed, notarized app `qrtz-echo.app` (`ai.qrtz.echo`, team
MR26CCZ496). Each install goes into `<data>/apps/echo/versions/<version>-<sha16>/`. The echo bundled in qrtz.app
stays the floor and the fallback.

Code: `crates/qrtz-app/src/echo_updates.rs` (new), `installed.rs` (`InstalledSpawn::pick_bundled`), `service.rs`
(the `bundledFallback` hook), `scripts/modules/mac-gate.sh` (check e6).

## Bozniak's requirements, and where each is met

| # | Requirement | Where |
|---|---|---|
| R1 | Own signed, notarized bundle `ai.qrtz.echo`, team MR26CCZ496; the requirement is compiled from the name and team, never from the feed or lock | `installed::launch_requirement` (unchanged) is passed to `select` and `install`; `MacBundleCheck`: `codesign --verify --deep --strict -R=<req> and notarized` plus spctl "Notarized Developer ID" |
| R2 / B3 | Suspended spawn, SecCode check by pid, then SIGCONT or SIGKILL, on every spawn | `InstalledSpawn::spawn` → `spawn_checked` (unchanged; it runs for a downloaded copy too) |
| R3 / B2 | Versions only go up; the highest accepted version is persisted | `apps/echo/high-water`, raised before the pointer swap and never lowered. `install` refuses anything not newer than the mark or the bundled echo; `select` ignores copies below the mark |
| R4 / B4 | sha256 against the signed index entry, fresh versioned dir, verify, atomic pointer swap, keep the last good copy, never overwrite a running binary | `install`: size and sha256 → unpack into `.staging-*` → identifier and `CFBundleVersion` must equal the index entry → bundle check → rename into place → `previous` ← `current`, `current` ← new (tmp + rename). Prune keeps both pointers. Existing dirs are never written |
| R5 | Bundled echo is the fallback; a download older than it is ignored | `select` skips anything not newer than the bundled manifest version; refusals fall back to `Pick::Bundled` |
| B1 | Pick the newest by `CFBundleVersion` inside the signed bundle | `select` orders by `bundle_info` (Info.plist, sealed by the signature that is checked before use), never by dir name, pointer or lock |
| B5 | Fallback writes a log line and shows a UI notice | `tracing::warn!(target: "qrtz::echo")`; on echo Running the host sets `modules.echo.bundledFallback` true, or false after an installed echo launches (`bundled_fallback_patch`). Renderers can't write the key (on main) |
| R6 | Entitlements microphone-only, checked by mac-gate | `entitlements_are_mic_only` in `MacBundleCheck`; mac-gate check (e6) runs on the bundled echo and every downloaded copy |
| R7 | Parakeet stays pinned by hash, separately | Not touched |

## Tests (`cargo test -p qrtz-app --lib echo_updates`)

- `a_valid_newer_copy_is_picked_by_the_version_inside_its_bundle`: the pointers swap and the previous copy is kept.
  When the pointers are swapped by hand, the newest by `CFBundleVersion` still wins.
- `a_downgrade_is_refused_at_install_and_ignored_at_launch`: an equal or lower version, or one below the bundled
  echo, is refused. A planted older copy is ignored at launch. The mark never drops. A bundled echo that overtook the
  download runs quietly.
- `a_tampered_copy_falls_back_to_the_bundled_echo_and_says_so`: also covers an unsigned download (not installed,
  mark unchanged) and a sha256 mismatch (refused before unpacking).
- `the_bundle_must_say_it_is_echo_at_the_index_version`
- `the_host_sets_and_clears_bundled_fallback`
- `entitlements_must_be_the_microphone_only`

## Not done yet (needed before it could ship)

- echo's own release pipeline (a signed, notarized `qrtz-echo.app` per version) and its entry in the signed index.
- The downloader that calls `echo_updates::install`, which today only the tests call.
- The `MacBundleCheck` macOS path compiles only under `cfg(target_os = "macos")` and hasn't run on a Mac yet.
- `select` runs only in release builds. Dev builds always run the bundled echo.
