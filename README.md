# qrtz

qrtz is a macOS app (Tauri 2, Rust core, React renderer, Swift helper) built from small modules that run on your Mac.
Speech recognition and summaries happen on-device: there are no cloud keys and no cloud LLM. The network is used to
download signed modules and speech models when you ask for them.

| Live recording                                     | Meeting summary                                                   |
| -------------------------------------------------- | ----------------------------------------------------------------- |
| ![Live transcript](docs/screenshots/note-live.png) | ![Summary and action items](docs/screenshots/meeting-summary.png) |

More screenshots are in [`docs/screenshots/`](docs/screenshots/).

## Modules

Modules are switched on and off in marketplace → installed. Each one lists the macOS permissions it needs next to its
switch, and nothing downloads until you turn a module on or press download in settings → speech.

- **echo:** dictation. Hold fn, speak, release, and the text is typed where your cursor is. Fully offline, using
  Parakeet TDT v2. Needs Microphone and Accessibility.
- **nano:** meeting and voice notes. Detects Zoom and Google Meet through the Accessibility tree, records, transcribes
  (Nemotron ASR), labels speakers (Nemotron diarizer) and writes a summary and action items on this Mac with Apple
  Intelligence (macOS 26+), falling back to a transcript-based summary. Notes are stored locally in SQLite with
  full-text search and Markdown export.
- **ctrl:** carries out one command at a time in the foreground by reading and acting on the macOS Accessibility tree.
  Esc stops it. It asks for consent and must be switched on before use.
- **snap:** window snapping with Spectacle's keyboard shortcuts. Needs Accessibility.
- **dash:** a read-only dashboard that your agents write to. Behind a dev flag; the server isn't live yet.

## Quickstart on a Mac

Needs Apple Silicon, macOS 14.2 or later, Xcode 16 or later, Rust (version pinned in `rust-toolchain.toml`) and
[bun](https://bun.sh) 1.4 or later.

```sh
git clone https://github.com/qrtz-ai/qrtz && cd qrtz
bun install
bun run dev:local
```

`bun run dev:local` builds every module (nano, snap, ctrl, echo) and the app from this checkout in debug mode and runs
it against a separate data directory. Flags (pass them after `--`):

```sh
bun run dev:local -- --without ctrl    # leave a module out (repeatable)
bun run dev:local -- --quit-installed  # quit /Applications/qrtz.app first
bun run dev:local -- --build-only      # build and print the environment, don't launch
```

The dev app runs as a separate app, `ai.qrtz.desktop.dev`, so macOS treats it as new: give it its own Accessibility and
Microphone grants when asked. Quit the installed qrtz first (or use `--quit-installed`), because both apps grab the fn
key and dictation breaks while both run. Details: [docs/dev/dev-local.md](docs/dev/dev-local.md).

`bun start` runs `tauri dev` with hot reload; there only echo comes from the checkout, and the other modules come from
the installed packages.

**No Mac?** Everything except real capture runs on Linux with the Rust mock helper:

```sh
QRTZ_MOCK_HELPER=1 QRTZ_MOCK_SCENARIO=standup QRTZ_MOCK_SPEED=0.1 bun start
```

**Logs** stay local, at `~/Library/Logs/qrtz/qrtz.log`. They never include transcripts, dictated text, notes, titles
or summaries. Set `QRTZ_LOG=debug` for more detail.

## Commands

```sh
bun run dev:local      # build and run everything from this checkout (macOS)
bun start              # tauri dev
bun run make           # tauri build (.app + .dmg on macOS)
bun run check          # lint + typecheck + vitest
bun run lint:fix       # oxlint --fix + oxfmt --write
bun run check:rust     # cargo fmt --check + clippy + cargo test --workspace
bun run gen:contracts  # re-render contracts/*.json from the zod contract
bun run e2e            # Playwright suite (needs Node for Playwright's runner)
bun run test:helper    # Swift helper tests
```

## Architecture

- **Rust core** (`crates/*`, `apps/desktop/src-tauri`): `qrtz-app` is the capability-checked IPC router; storage is
  SQLite with FTS5.
- **Swift helper** (`native/qrtz-helper`): Accessibility, audio capture and the module binaries.
- **Renderer** (`apps/desktop`, `packages/*`): `packages/core` holds the zod IPC contract (rendered to `contracts/*.json`),
  `packages/ui` is the design system, and each view in `packages/features/*` composes `@qrtz/ui` only. `bun run lint`
  enforces these boundaries.
- **Modules** (`modules/*`): each declares its channels, capabilities, permissions and model dependencies in
  `qrtz.json`.

Docs: [architecture](docs/architecture.md) · [rust core api](docs/rust-core-api.md) ·
[helper protocol](docs/helper-protocol.md) · [security](docs/security.md) · [design system](docs/design-system.md) ·
[release](docs/release.md)

## License

See [LICENSE](LICENSE). Third-party components keep their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Speech models (Parakeet, Nemotron) are not bundled; they are
downloaded separately and are covered by their own licenses.
