# Agent Dash: local test setup

Run the dash server and the dev app on your Mac only. Nothing here deploys, uses a Cloudflare account, creates
secrets or touches DNS. **Never run `wrangler deploy`**: `services/dash-worker/wrangler.toml` holds the
dash.qrtz.ai route.

## Needs

- Node 22 or later on PATH (wrangler 4.151 refuses Node 20), bun, Rust and the usual Tauri dev setup.
- `cd services/dash-worker && bun install` once (`dash:dev` also does it if needed).

## Run it

| command                                        | what it does                                                                                                                                                    |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `bun run dash:e2e`                             | Self-check. It starts its own throwaway server on 127.0.0.1:8798, runs setup, code, enroll, approve, heartbeat, widget and owner view, then deletes everything. |
| `bun run dash:dev` (`-- --wipe` to start over) | Runs the server at http://127.0.0.1:8799 with `wrangler dev --local`. It listens on loopback only and keeps state in `services/dash-worker/.wrangler/state`.    |
| `bun run dash:token`                           | Copies the one-time setup token to the clipboard without printing it.                                                                                           |
| `bun run dash:app`                             | Runs the dev app with Dash shown and its helper pointed at the local server.                                                                                    |

1. In terminal 1, run `bun run dash:e2e`. It should end with `dash:e2e: all passed`.
2. Still in terminal 1, run `bun run dash:dev` and leave it running.
3. In terminal 2, run `bun run dash:app`. The dev app (`ai.qrtz.desktop.dev`) uses its own data dir and leaves
   `/Applications/qrtz.app` alone.
4. Run `bun run dash:token`. Then in the app, open Dash › settings › server, paste the token and connect. The helper
   saves the owner key in the Keychain item `qrtz-dash-dev`, never in the real `ai.qrtz.dash-net` item. The token
   works once. A second setup answers 410.
5. Issue an enrollment code in the app. The local server's codes start with `TST-`, and production refuses that
   prefix. Enroll an agent with the code, for example:
   `curl -s -X POST http://127.0.0.1:8799/v1/enroll -H 'content-type: application/json' -d '{"code":"TST-…","name":"test bot"}'`
6. The agent shows as pending. Approve it in the app. Its heartbeats and widgets then appear, for example:
   `curl -s -X POST http://127.0.0.1:8799/v1/widgets -H "authorization: Bearer <agent_token>" -H 'content-type: application/json' -d '{"key":"t","type":"list","title":"links","state":{"items":[{"text":"qrtz","url":"https://qrtz.ai"}]}}'`
7. Disconnect in the app. That deletes the `qrtz-dash-dev` Keychain item.

## What keeps this local (for review)

- **Server** (`services/dash-worker/src/dev.ts`): local test mode accepts plain http and issues `TST-` codes. It
  turns on only when all three of these hold:
  - `DASH_DEV_LOCAL=1` is set. That variable exists only in the gitignored `.dev.vars` (mode 600) that `dash:dev`
    writes. `wrangler.toml` never sets it, and a test checks that.
  - The peer is loopback.
  - The host is `127.0.0.1`.

  Production answers http with 400 and refuses `TST-` codes the same way it refuses a wrong code, counting them as
  wrong codes too. The enroll limits are unchanged, and the scanner and scrubbing stay on.

- **App** (`crates/qrtz-dash-net`): `QRTZ_DASH_ORIGIN` is read only in debug builds with the `dev-local` feature.
  Release builds have no override code at all, which a test checks with `--release`. The only value accepted is
  `http://127.0.0.1:<port>`. Egress is still pinned to that one origin, with no redirects and no proxy. qrtz-app
  passes the variable to the helper only in debug + dev-local builds, and the helper exits if the variable is
  invalid.
- **Secrets**: the pepper and setup token are random, generated locally and never printed. They stay valid only as
  long as the local state does. `--wipe` deletes both.
