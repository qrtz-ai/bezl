# ADR 0237: snap, nano and ctrl ship inside qrtz.app, like echo

Status: **accepted** (2026-10-23, QRT-361; Bozniak's review answers below). Amends ADR 0169 ("qrtz ships empty") and ADR
0174 ("echo ships inside qrtz.app; nano, ctrl and snap stay in the marketplace"). Leaves ADR 0171 (models from Hugging
Face), ADR 0185 (echo's own updates) and ADR 0231 (Agent Dash's network helper is never bundled) as they are.

## Context

Since ADR 0169, only the native helper of a first-party module is a downloadable package. Its frontend never was: the
views (`@qrtz/feature-snap`, `-note`, `-ctrl`) and the host code that drives each helper (`crates/qrtz-app/src/snap.rs`,
`module_helpers.rs`, the `snap.configure` / ctrl / nano lines) are compiled into qrtz.app, because module views stay in
the signed shell (`manifest.rs` `views`, ADR 0078 CSP). So every module is split in two halves that ship separately:

| half                                | ships with          | versioned by                         |
| ----------------------------------- | ------------------- | ------------------------------------ |
| view + host driver + settings shape | qrtz.app            | app version                          |
| `qrtz-<id>` helper                  | marketplace package | `modules/<id>/module.json` `version` |

The two drift, and nothing catches it:

- The only check that runs is `minApp` / `maxApp` (`qrtz-modules/src/launcher.rs` `in_range`). `"protocol": 1` is
  declared in every manifest (`manifest.rs:41`) and read by no one.
- Helpers ignore message types and fields they don't know (`SnapProtocol.swift` `.ignored`). A setting added in the app
  shows in the UI and silently does nothing on an older helper; a helper fix waits on a separate publish.
- Found on 2026-10-23 (QRT-359): the Safari snap fix lives entirely in `qrtz-snap`, yet the repo's snap 0.1.2 was never
  published (no `snap-v0.1.2` tag), so this Mac ran 0.1.1 against a newer app. Diagnosing it meant first working out
  which half was which version.

echo already avoids this: since ADR 0174 it is `Contents/MacOS/qrtz-echo`, signed `ai.qrtz.echo`, covered by the app's
notarization, launched through `installed::bundled_entry`. Most of that path is generic over
`qrtz_modules::BUNDLED_EXTENSIONS`, not hardcoded to echo.

## Decision

1. `BUNDLED_EXTENSIONS = ["echo", "nano", "ctrl", "snap"]`. Each helper is `Contents/MacOS/qrtz-<id>`, `launch.kind:
   "bundled-helper"`, signed with its own identifier (`ai.qrtz.<id>`) and its own entitlements from its `module.json`,
   covered by the app's notarization. No package, no index entry, no lock entry.
2. A module's view, host driver and helper always come from the same commit. The app version is the only version a user
   can have; `module.json` `version` stays as the helper's own build label (reported in diagnostics), not an install
   unit.
3. Unchanged:
   - **Models still download.** nano's three models and ctrl's optional laya model come from their pinned Hugging Face
     URLs on first enable, sha256-checked (ADR 0171), as echo's Parakeet does today. App size grows by code only.
   - **Switches and consent.** Bundled modules stay off until switched on. ctrl's consent flow, the `experimentalApps`
     gate for nano/ctrl (QRT-352, 0.3.4), TCC prompts on first use, and per-module `enabled` all stay as they are.
   - **The launch check.** Each bundled helper is checked with `codesign -R` against the requirement rebuilt from its
     name and team MR26CCZ496 (`installed::launch_requirement`) before every spawn, only from a real
     `.app/Contents/MacOS` (confinement review 18392a62).
   - **The marketplace** stays for third-party modules and models. Agent Dash's network helper is never bundled (ADR
     0231).
4. echo's own update channel (ADR 0185) stays echo-only. Nothing new gets one.

## Consequences

- **App size:** 45 MB today. The release helpers are nano ≈20.4 MB, ctrl ≈0.9 MB, snap ≈0.4 MB
  (`native/qrtz-helper/.build/out/Products/Release`, 2026-10-23), so ≈+22 MB, ≈67 MB total. nano and echo are each
  ≈20 MB and probably duplicate shared Swift code; a shared framework could recover some of that later (not in scope).
- **A helper fix becomes an app release.** QRT-359 ships in the next qrtz.app, not as snap 0.1.2.
- **Fewer moving parts:** no per-module notarization record, publish step, index entry, lock entry or revocation path
  for these three. Fewer states to support (no "new app, old nano").
- **The marketplace shrinks** to models and third-party modules. Its first-party listings go away.
- **Existing installs need a migration** (below). It is one-way: once an app with this ADR has launched, the old
  packages are no longer used.

## Migration for users with packages installed

- On first launch of the bundling app, the lock entries for `nano`, `ctrl`, `snap` are dropped and the bundled copy is
  used. `modules.<id>.installed` becomes true (as for echo), and `enabled` keeps the user's value, so whatever was on
  stays on.
- Their models stay. They are refcounted packages and remain referenced by the (now bundled) module.
- `packages/<id>/<version>/` dirs for the three are pruned after the bundled helper has launched once, with the same
  prune rules as echo's (keep anything running).
- Already-published packages stay on the index for older apps. The last published version of each gets
  `maxApp` = the last app version without this ADR, so an older app never sees a package newer than itself, and a newer
  app never installs one.

## Change list

Gated paths (signing, release, entitlements, permissions, process spawning, version bumps, manifest format) are marked
**[gated]**. echo's bundling commit (10fbd422, 42 files) is the template.

### Manifests and the module crate

- `modules/{nano,ctrl,snap}/module.json`: `launch.kind` → `bundled-helper`, `launch.entry` → `qrtz-<id>` (plain file
  name). **[gated: manifest format]**
- `modules/modules.lock.json`: drop the three package entries. **[gated: manifest format]**
- `modules/{nano,ctrl}/qrtz.json`: keep model dependencies, drop package publish metadata.
- `crates/qrtz-modules/src/lib.rs`: `BUNDLED_EXTENSIONS` gains `nano`, `ctrl`, `snap`. Tests: manifests parse as
  bundled, `is_bundled_extension` for each.
- `contracts/module-manifest.schema.json`: no change expected (the kind exists); confirm.

### Build and bundle **[gated: release]**

- `scripts/build-helper.mjs`: build and stage `qrtz-nano`, `qrtz-ctrl`, `qrtz-snap` as release binaries, like
  `qrtz-echo`.
- `apps/desktop/src-tauri/tauri.macos.conf.json`: `externalBin` gains `binaries/qrtz-nano`, `binaries/qrtz-ctrl`,
  `binaries/qrtz-snap`.
- `apps/desktop/src-tauri/build.rs`: stage the new binaries (as it does `qrtz-echo`).
- `scripts/modules/build-packages.sh`: stop building the three as packages; keep the requirement check (b904bf91) for
  the bundled binaries in `bundled-helpers.sh` instead.

### Signing and verification **[gated: signing, entitlements]**

- `scripts/release/sign-mac.sh`: sign each from `bundled_helpers` output (identifier + entitlements per module.json):
  nano `com.apple.security.device.audio-input` and `com.apple.security.personal-information.calendars` (see review
  answer b); ctrl and snap none.
- `scripts/release/bundled-helpers.sh` + `bundled-helpers.test.sh`: the real-modules row count goes 1 → 4, and the
  allowlist test covers each.
- `scripts/release/verify-bundle.sh` + `verify-bundle.test.mjs`: the Mach-O allowlist grows from three executables to
  six; exact entitlement allowlists for `qrtz-nano` (audio-input, calendars), `qrtz-ctrl` (none), `qrtz-snap` (none); the dev-local
  override-string scan (line ~216) covers the new names.
- `docs/modules/confinement-map.json`: `bundledCode` entries for the three.
- `scripts/dev/sign.sh`, `scripts/dev/dev-sign.test.mjs`: dev signing of the new binaries.

### Launch and host **[gated: process spawning]**

- `crates/qrtz-app/src/installed.rs`: `bundled_entry` / `launch_requirement` already key off `is_bundled_extension`;
  add `bundled_<id>_is_checked_against_its_bundled_manifest` tests for each. Line 397's `module != "echo"` (ADR 0185's
  self-update) stays echo-only.
- `crates/qrtz-app/src/module_helpers.rs`: `HelperLayout::Installed.bundled` resolves the three from `Contents/MacOS`.
- `crates/qrtz-app/src/snap.rs`: `spawner` (line ~54) currently assumes snap's installed package in the shipping app;
  point it at the bundled entry.
- `apps/desktop/src-tauri/src/paths.rs`: bundled programs for the three.
- `crates/qrtz-app/src/packages_ipc.rs`: already filters bundled names out of `packages:list` / `packages:plan`
  (lines 229, 252); covered by the list change.
- `crates/qrtz-app/src/modules.rs`: `installed` written for bundled modules (as for echo) and the
  `config:set` / `speech:status` view rules extended to the three where echo has them.
- Migration (above): lock cleanup + prune, in `installed.rs` / the lock loader, with a test for an old lock holding all
  three packages.

### Frontend

- `packages/core/src/modules.ts` + `modules.test.ts`: sidebar visibility for bundled modules (echo's `isViewOn`
  rule: row shows while off, the view holds the switch).
- `packages/features/{note,ctrl,snap}`: the view's off-state holds the switch and, for nano/ctrl, asks once to download
  the models (as `TalkView.tsx` does for Parakeet). Remove "install from marketplace" states.
- Marketplace view: drop the first-party listings.

### Publishing and hosting

- `tools/qrtz-publish`: stop publishing the three. Set `maxApp` on their last published versions (migration).
  **[gated: release]**
- `docs/modules/{hosting-setup,production-setup}.md`, `docs/release.md` checklist.

### Tests and CI **[gated: `.github/`]**

- `crates/qrtz-lifecycle/tests/offline_graph.rs`: identifier and bundled tests for the three.
- `crates/qrtz-app/tests/{empty_shell,marketplace,module_hosting,module_update,upgrade_requests,ctrl_install}.rs`:
  `empty_shell` no longer empty for these; `ctrl_install` becomes a models-only install.
- `.github/workflows/ci.yml`: the macOS unsigned release build + `verify-bundle` job checks six executables.

### Docs

- PROGRESS.md entry, CHANGELOG ("nano, ctrl and snap ship inside qrtz.app again"), `decisions-parity.md` row,
  ADR 0169 / 0174 status lines.

## Questions for Bozniak

1. Any objection to the three getting the same launch check as echo (`codesign -R` per spawn, requirement from the
   compiled-in manifest), with no cdhash pinning since there's no downloaded copy?
2. nano carries audio-input plus screen-recording/accessibility TCC use. Is bundling it under the app's notarization
   (instead of its own notarization record) acceptable for the confinement map?
3. Migration: is pruning old package dirs after one bundled launch enough, or should they stay until the next app
   update?
4. Should `"protocol"` become enforced anyway (for third-party modules), or is that separate work?

## Review answers (Bozniak)

a. **Launch check.** The three get exactly echo's check: `codesign -R` against the requirement rebuilt from the
compiled-in manifest (identifier `ai.qrtz.<id>`, team MR26CCZ496) before every spawn, only from a real
`.app/Contents/MacOS`. There is no cdhash pin, since no copy is ever downloaded. Tests:
`installed::bundled_nano_ctrl_and_snap_are_checked_against_their_bundled_manifests`.
b. **Notarization and entitlements.** Bundled under the app's notarization. Each binary's exact entitlement set is
enforced by `verify-bundle.sh` (exact match, not a subset): `qrtz-nano` audio-input + calendars, `qrtz-ctrl` and
`qrtz-snap` none (`bundledCode` `"entitlements": null` in the confinement map). **Deviation from the draft:** the
draft listed nano as audio-input only, but `modules/nano/module.json` has always also declared
`com.apple.security.personal-information.calendars` (meeting detection). The bundled signature keeps both rather
than silently dropping a permission nano uses; removing calendars would be a separate product decision.
Screen recording and Accessibility are TCC grants, not entitlements, and stay prompt-on-first-use.
c. **nano and ctrl stay experimental.** The `experimentalApps` gate stays the primary barrier: with it off, the host
never spawns either helper, whatever `modules.<id>.enabled` says. Test:
`e2e_split_host::a_fresh_install_never_spawns_nano_or_ctrl` (no config, and experimental apps off with both switched
on) plus `modules::the_bundled_nano_ctrl_and_snap_views_hold_only_their_own_switch`.
d. **Name squatting.** A package named `echo`, `nano`, `ctrl` or `snap` is refused from an index, from a lock and from
a sideload (`InstallError::Bundled`; `packages:plan` says it is built into qrtz.app). Test:
`module_hosting::a_package_named_like_a_bundled_module_is_refused_from_an_index_and_a_sideload`.
e. **Pruning.** An old lock's entries for the three are retired on first launch but their package dirs are kept until
that module's bundled helper has passed its launch check and spawned once; only then are they released for prune.
Test: `module_hosting::an_old_locks_nano_is_retired_kept_until_the_bundled_helper_launches_then_pruned`.
f. **`protocol` enforcement** for third-party modules is separate work: QRT-363.

## Not covered

- Measuring the signed, notarized app size (the numbers above are unsigned release builds).
- A shared framework for nano/echo.
- Third-party module skew (the `protocol` check).
