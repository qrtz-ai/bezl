# Agent Dash: local test setup

Run the dash server and the dev app on your Mac only. Nothing here deploys, uses a Cloudflare account, creates
secrets or touches DNS. **Never run `wrangler deploy`**: `services/dash-worker/wrangler.toml` holds the
dash.qrtz.ai route.

## Needs

- Node 22 or later on PATH (wrangler 4.151 refuses Node 20), bun, Rust and the usual Tauri dev setup.
- `cd services/dash-worker && bun install` once (`dev:local` also does it if needed).

## Run it

| command                                                                     | what it does                                                                                                                                                    |
| --------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `bun run --cwd services/dash-worker e2e`                                    | Self-check. It starts its own throwaway server on 127.0.0.1:8798, runs setup, code, enroll, approve, heartbeat, widget and owner view, then deletes everything. |
| `bun run --cwd services/dash-worker dev:local` (add `--wipe` to start over) | Runs the server at http://127.0.0.1:8799 with `wrangler dev --local`. It listens on loopback only and keeps state in `services/dash-worker/.wrangler/state`.    |
| `bun run --cwd services/dash-worker token`                                  | Copies the one-time setup token to the clipboard without printing it.                                                                                           |
| `bun run --cwd services/dash-worker app`                                    | Runs the dev app with Dash shown and its helper pointed at the local server.                                                                                    |

1. In terminal 1, run `bun run --cwd services/dash-worker e2e`. It should end with `e2e: all passed`.
2. Still in terminal 1, run `bun run --cwd services/dash-worker dev:local` and leave it running.
3. In terminal 2, run `bun run --cwd services/dash-worker app`. The dev app (`ai.qrtz.desktop.dev`) uses its own data dir and leaves
   `/Applications/qrtz.app` alone.
4. Run `bun run --cwd services/dash-worker token`. Then in the app, open Dash › settings › server, paste the token and connect. The helper
   saves the owner key in the Keychain item `qrtz-dash-dev`, never in the real `ai.qrtz.dash-net` item. The token
   works once. A second setup answers 410.
5. Get an enrollment code. Codes issued in the app start with `ENR-`. For a test-only code, call the endpoint that
   exists only in local mode (production answers it with 404). It needs the owner key, which lives in the helper's
   Keychain item, so `e2e` shows the whole flow on its own server. Enroll an agent with a code, for example:
   `curl -s -X POST http://127.0.0.1:8799/v1/enroll -H 'content-type: application/json' -d '{"code":"ENR-…","name":"test bot"}'`
6. The agent shows as pending. Approve it in the app. Its heartbeats and widgets then appear, for example:
   `curl -s -X POST http://127.0.0.1:8799/v1/widgets -H "authorization: Bearer <agent_token>" -H 'content-type: application/json' -d '{"key":"t","type":"list","title":"links","state":{"items":[{"text":"qrtz","url":"https://qrtz.ai"}]}}'`
7. Disconnect in the app. That deletes the `qrtz-dash-dev` Keychain item.

## What keeps this local (for review)

- **Server** (`services/dash-worker/src/dev.ts`): local test mode accepts plain http and serves `POST /v1/owner/test-codes` (`TST-` codes). It
  turns on only when all three of these hold:
  - `DASH_DEV_LOCAL=1` is set. That variable exists only in the gitignored `.dev.vars` (mode 600) that `dev:local`
    writes. `wrangler.toml` never sets it, and a test checks that.
  - The peer is loopback.
  - The host is `127.0.0.1`.

  Production answers http with 400, answers `/v1/owner/test-codes` with 404, and refuses `TST-` codes down the
  normal path (lock check first, then counted as a wrong code). The enroll limits are unchanged, and the scanner and scrubbing stay on.

- **App** (`crates/qrtz-dash-net`): `QRTZ_DASH_ORIGIN` is read only in debug builds with the `dev-local` feature.
  Release builds have no override code at all, which a test checks with `--release`. The only value accepted is
  `http://127.0.0.1:<port>`. Egress is still pinned to that one origin, with no redirects and no proxy. qrtz-app
  passes the variable to the helper only in debug + dev-local builds, and the helper exits if the variable is
  invalid.
- **Secrets**: the pepper and setup token are random, generated locally and never printed. They stay valid only as
  long as the local state does. `--wipe` deletes both.

## Old Keychain items from before origin tagging (QRT-286)

Since f9c8518 every owner key is stored with its origin as the Keychain item's comment, and the comment is part of
every lookup. An item saved by a dev build from before that commit (service `ai.qrtz.dash-net`, no comment) can no
longer be read or deleted by the app, so it just sits in the login Keychain.

No such item exists outside dev Macs: Dash has never shipped (qrtz-dash-net isn't in v0.1.0 or v0.2.0) and
dash.qrtz.ai has never been deployed, so no release build ever saved an owner key. The app therefore has no
first-run sweep for these items. If you ran Dash in a dev build before f9c8518, remove them by hand once:

```sh
# Repeat each line until it says the item could not be found.
security delete-generic-password -s ai.qrtz.dash-net -a owner-key
security delete-generic-password -s ai.qrtz.dash-net -a owner-key.pending
```

This also removes any tagged `ai.qrtz.dash-net` item on that Mac. That is harmless on a dev Mac: connect again with a
new setup token. Local test mode never uses that service (it uses `qrtz-dash-dev`), so it isn't affected.

If Dash ever ships in a release build that could have saved an untagged item, this stops being enough: add a
first-run sweep that deletes only items with no comment, with a test, before that release.
