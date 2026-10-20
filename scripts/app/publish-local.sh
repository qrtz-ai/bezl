#!/usr/bin/env bash
# ADR 0181: publish one qrtz.app release from Jared's Mac exactly the way app-release.yml does after its build, or
# re-sign the live feed the way app-feed-refresh.yml does (--refresh). While GitHub Actions is off, this is the release.
#
#   scripts/app/publish-local.sh [--app <qrtz.app>] [--dmg <dmg>] [--no-tag] [--dry-run]
#   scripts/app/publish-local.sh --refresh [--dry-run]
#
# Publish, after `bun run release:mac` built, signed, notarized and stapled qrtz.app and its .dmg for the version in
# apps/desktop/src-tauri/tauri.conf.json (<v>):
#   1. Refuses unless the tree is clean, HEAD is on origin's main, app-v<v> exists neither here nor on origin, the live
#      v1/app/latest.json is below <v> (or not there yet) and nothing is up under v1/app/<v>/. Published objects are
#      immutable: a version that went up, even partly, is never published again. Bump it (scripts/app/version.mjs bump).
#   2. The bundle is qrtz <v> (ai.qrtz.desktop) with qrtz-echo, signed by team MR26CCZ496 and notarized (the
#      app-release.yml checks). The .app is found in $CARGO_TARGET_DIR, target/ or apps/desktop/src-tauri/target/
#      (release/bundle/macos/qrtz.app); the .dmg is qrtz_<v>_aarch64.dmg next to it.
#   3. Tars it (COPYFILE_DISABLE=1) into qrtz_<v>_aarch64.app.tar.gz, signs that (`tauri signer sign --app-version`),
#      verifies the signature with minisign against crates/qrtz-app/src/app_update_key.pub, writes the feed
#      (scripts/app/feed.mjs --expires-days 30, notes from scripts/release-notes.mjs) and signs it into latest.json
#      (scripts/app/sign-feed.sh).
#   4. scripts/app/upload.sh (the versioned files, then qrtz-latest.dmg, latest.json last), scripts/app/verify-live.sh,
#      then tags app-v<v> and pushes the tag (scripts/app/version.mjs tag --push) unless --no-tag.
# --refresh (app-feed-refresh.yml): checks that the live latest.json verifies against the committed public key,
#   re-signs it unchanged but for a new expires 30 days out, uploads only latest.json (60 s cache) and waits until
#   modules.qrtz.ai serves it. No feed live yet: nothing to do. Run it before the live feed is 30 days old.
# --dry-run: the same read-only checks (git, and the live feed over HTTPS), then every step printed as `+ …`. Nothing
#   else: no key or keychain read, no file written, no upload, no tag. Runs on any OS (scripts/app/publish-local.test.mjs).
#
# Secrets: never printed, never on a command line, never traced (this script turns `set -x` off), only in the
# environment of the one command that needs them:
#   QRTZ_APP_UPDATE_KEY  the app-update private key file, default ~/.qrtz-keys/app-update.key (made by
#                        scripts/ci/setup-secrets.sh app-update, 0600, no password: TAURI_SIGNING_PRIVATE_KEY_PASSWORD
#                        is empty, as in CI). Read into TAURI_SIGNING_PRIVATE_KEY for `tauri signer sign` and
#                        sign-feed.sh only.
#   R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY  used as they are when the key id and secret are set;
#                        otherwise from the login keychain item qrtz-r2 (account = the access key id). The account id
#                        (not secret) is $R2_ACCOUNT_ID, ~/.qrtz-keys/r2-account-id or the public default
#                        (docs/release.md). Read only for the upload, in a subshell.
# Other env: QRTZ_APP_HOST (default https://modules.qrtz.ai), R2_BUCKET (default qrtz-modules), QRTZ_KEYS
# (default ~/.qrtz-keys). The work dir (only files that go public) is removed after a good run, kept after a failure.
#
# Runs on Jared's Mac only (CI does the same in app-release.yml): the app-update key never touches a server-side box.
set -euo pipefail
{ set +x; } 2>/dev/null

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
host="${QRTZ_APP_HOST:-https://modules.qrtz.ai}"
keys="${QRTZ_KEYS:-$HOME/.qrtz-keys}"
keyfile="${QRTZ_APP_UPDATE_KEY:-$keys/app-update.key}"
r2_account_default="177665085902fbdae8b82993bb7a0b91"
pubkey="$repo/crates/qrtz-app/src/app_update_key.pub"
tauri="$repo/node_modules/.bin/tauri"
dry=0 refresh=0 tag=1 app="" dmg=""
# A key someone exported earlier is never the one used: it comes from $keyfile, for one command at a time.
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD

die() { echo "publish-local: $*" >&2; exit 1; }
say() { echo "== $*"; }
# dev-local is never published: not from a dev-local shell, and not when the HEAD build turns the feature on by
# default (verify-bundle.sh also refuses a binary that carries its strings).
[[ -n "${QRTZ_DEV_REPO:-}${QRTZ_DEV_LOCAL:-}" ]] && die "dev-local is enabled in this shell (QRTZ_DEV_REPO set): refused"
for t in "$repo/apps/desktop/src-tauri/Cargo.toml" "$repo/crates/qrtz-app/Cargo.toml"; do
  grep -Eq '^default *= *\[[^]]*dev-local' "$t" && die "${t#"$repo"/} enables dev-local by default: refused"
done
run() { # <command…>: run it, or print it (--dry-run)
  if [[ $dry == 1 ]]; then
    printf '+'; printf ' %q' "$@"; printf '\n'
  else
    "$@"
  fi
}
run_to() { # <out file> <command…>: run it with stdout to the file, or print it
  local out="$1"; shift
  if [[ $dry == 1 ]]; then
    printf '+'; printf ' %q' "$@"; printf ' > %q\n' "$out"
  else
    "$@" >"$out"
  fi
}
with_key() { # [--quiet] <command…>: run it with TAURI_SIGNING_PRIVATE_KEY from $keyfile in its environment only
  local quiet=0
  [[ "$1" == --quiet ]] && { quiet=1; shift; }
  if [[ $dry == 1 ]]; then
    printf '+ TAURI_SIGNING_PRIVATE_KEY=<%s> TAURI_SIGNING_PRIVATE_KEY_PASSWORD=' "$keyfile"
    printf ' %q' "$@"; printf '\n'
    return 0
  fi
  if [[ $quiet == 1 ]]; then
    TAURI_SIGNING_PRIVATE_KEY="$(tr -d '[:space:]' <"$keyfile")" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" "$@" >/dev/null
  else
    TAURI_SIGNING_PRIVATE_KEY="$(tr -d '[:space:]' <"$keyfile")" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" "$@"
  fi
}
with_r2() { # <command…>: run it with the R2 credentials and the qrtzr2: remote (scripts/ci/r2-env.sh), in a subshell
  if [[ $dry == 1 ]]; then
    printf '+ R2_*=<%s>' "$( [[ -n "${R2_ACCESS_KEY_ID:-}" && -n "${R2_SECRET_ACCESS_KEY:-}" ]] && echo env || echo 'keychain qrtz-r2')"
    printf ' %q' "$@"; printf '\n'
    return 0
  fi
  (
    if [[ -z "${R2_ACCESS_KEY_ID:-}" || -z "${R2_SECRET_ACCESS_KEY:-}" ]]; then
      R2_ACCESS_KEY_ID="$(security find-generic-password -s qrtz-r2 2>/dev/null | sed -n 's/^ *"acct"<blob>="\(.*\)"$/\1/p')"
      [[ -n "$R2_ACCESS_KEY_ID" ]] || die "no keychain item qrtz-r2 (security add-generic-password -s qrtz-r2 -a <access key id> -w)"
      R2_SECRET_ACCESS_KEY="$(security find-generic-password -s qrtz-r2 -w)" || die "can't read the keychain item qrtz-r2"
    fi
    if [[ -z "${R2_ACCOUNT_ID:-}" ]]; then
      if [[ -f "$keys/r2-account-id" ]]; then R2_ACCOUNT_ID="$(tr -d '[:space:]' <"$keys/r2-account-id")"; else R2_ACCOUNT_ID="$r2_account_default"; fi
    fi
    export R2_ACCOUNT_ID R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY
    . "$repo/scripts/ci/r2-env.sh"
    "$@"
  )
}
sha() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }
gt() { # <a> <b>: a > b (x.y.z)
  # Arguments go in the environment: under `bun --bun` (the tests), `node -e` drops all but its last argument.
  GT_A="$1" GT_B="$2" node -e 'const p=(v)=>v.split(".").map(Number),a=p(process.env.GT_A),b=p(process.env.GT_B);for(let i=0;i<3;i++)if(a[i]!==b[i])process.exit(a[i]>b[i]?0:1);process.exit(1)'
}
abs() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
http() { # <curl args…>: print the HTTP status (000 when unreachable)
  curl -sS --max-time 60 -H 'Cache-Control: no-cache' -w '%{http_code}' "$@" 2>/dev/null || true
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app="$(abs "${2:?--app needs the path to qrtz.app}")"; shift 2 ;;
    --dmg) dmg="$(abs "${2:?--dmg needs a path}")"; shift 2 ;;
    --no-tag) tag=0; shift ;;
    --refresh) refresh=1; shift ;;
    --dry-run) dry=1; shift ;;
    -h|--help) sed -n '2,41p' "$0"; exit 0 ;;
    *) die "unknown argument: $1 (--app <qrtz.app>, --dmg <dmg>, --no-tag, --refresh, --dry-run)" ;;
  esac
done
[[ $refresh == 0 || ( -z "$app" && -z "$dmg" && $tag == 1 ) ]] || die "--refresh takes no --app, --dmg or --no-tag"

# ---- checks that hold in every mode (read only) ----
cd "$repo"
if [[ $dry == 0 ]]; then
  [[ "$(uname -s)" == Darwin ]] || die "runs on Jared's Mac only (the app-update key never touches a server-side box)"
  for t in node curl minisign rclone security tar git; do command -v "$t" >/dev/null || die "missing $t (brew install minisign rclone)"; done
  [[ -x "$tauri" ]] || die "no tauri CLI at $tauri (bun install --frozen-lockfile)"
fi
[[ -z "$(git status --porcelain)" ]] || die "the tree isn't clean (commit or stash first; a release is a commit on main)"
node scripts/app/feed.mjs --check-key "$pubkey" || die "$pubkey holds no app-update public key (scripts/ci/setup-secrets.sh app-update)"
case "$keyfile" in *qrtz-root*) die "$keyfile is in a root key dir: refused" ;; esac
[[ -f "$keyfile" ]] || die "no app-update key at $keyfile (scripts/ci/setup-secrets.sh app-update, or set QRTZ_APP_UPDATE_KEY)"
if [[ $dry == 0 ]]; then
  [[ -r "$keyfile" && -s "$keyfile" ]] || die "$keyfile isn't readable"
  mode="$(stat -f %Lp "$keyfile")"
  [[ "$mode" == 600 || "$mode" == 400 ]] || die "$keyfile is mode $mode, not 600 (chmod 600 it)"
fi
scratch="$(mktemp -d)"
work=""
cleanup() {
  local rc=$?
  rm -rf "$scratch"
  if [[ -n "$work" ]]; then
    if [[ $rc == 0 ]]; then
      rm -rf "$work"
    else
      echo "publish-local: failed; the work dir is kept: $work (anything already uploaded is immutable: never re-sign" >&2
      echo "  it; check it with VERSION=<v> scripts/app/verify-live.sh $work <dmg>, or bump the version)" >&2
    fi
  fi
}
trap cleanup EXIT
live_version() { # print the live feed's version, or nothing when there is no feed (404); dies on anything else
  local code
  code="$(http -o "$scratch/live.json" "$host/v1/app/latest.json?r=$$-$RANDOM")"
  case "$code" in
    200) node scripts/app/feed.mjs --unwrap "$scratch/live.json" |
      node -e 'let s="";process.stdin.on("data",(d)=>(s+=d)).on("end",()=>console.log(JSON.parse(s).version))' ;;
    404) rm -f "$scratch/live.json" ;;
    *) die "$host/v1/app/latest.json: HTTP $code (can't tell what is live; nothing done)" ;;
  esac
}

# ---- --refresh: app-feed-refresh.yml ----
if [[ $refresh == 1 ]]; then
  lv="$(live_version)"
  [[ -n "$lv" ]] || { echo "publish-local: no feed on $host yet: nothing to refresh"; exit 0; }
  say "the live feed (qrtz $lv) is ours"
  if [[ $dry == 0 ]]; then work="$(mktemp -d)"; else work=""; fi
  d="${work:-<work>}"
  if [[ $dry == 1 ]]; then
    echo "+ unwrap $host/v1/app/latest.json > $d/live-feed.json $d/live.sig"
  else
    F="$scratch/live.json" O="$d/live-feed.json" G="$d/live.sig" node -e 'const fs=require("fs"),{F,O,G}=process.env,e=JSON.parse(fs.readFileSync(F));fs.writeFileSync(O,Buffer.from(e.feed,"base64"));fs.writeFileSync(G,Buffer.from(e.signature,"base64"))'
    cp "$scratch/live.json" "$d/live.json"
    grep -v '^#' "$pubkey" | tr -d '[:space:]' | base64 --decode >"$d/k.pub"
  fi
  run minisign -V -p "$d/k.pub" -x "$d/live.sig" -m "$d/live-feed.json"
  say "re-sign with a new expires (30 days)"
  run_to "$d/feed.json" node scripts/app/feed.mjs --unwrap "$d/live.json" --expires-days 30
  with_key bash scripts/app/sign-feed.sh "$d/feed.json" "$d/latest.json"
  say "upload latest.json and check"
  with_r2 rclone copyto "$d/latest.json" "qrtzr2:${R2_BUCKET:-qrtz-modules}/v1/app/latest.json" --ignore-times \
    --header-upload "Cache-Control: public, max-age=60" --header-upload "Content-Type: application/json" -v
  if [[ $dry == 1 ]]; then
    echo "+ curl $host/v1/app/latest.json until it is $d/latest.json (20 tries, 15 s apart)"
    exit 0
  fi
  want="$(sha "$d/latest.json")"
  for i in $(seq 1 20); do
    if curl -fsS -H 'Cache-Control: no-cache' -o "$d/got" "$host/v1/app/latest.json?r=$$-$i" && [[ "$(sha "$d/got")" == "$want" ]]; then
      echo "publish-local: the re-signed feed (qrtz $lv) is live"
      exit 0
    fi
    sleep 15
  done
  die "$host/v1/app/latest.json isn't the re-signed feed"
fi

# ---- publish: app-release.yml after the build ----
v="$(node scripts/app/version.mjs current)"
[[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "tauri.conf.json's version isn't x.y.z: $v"
git rev-parse -q --verify "refs/tags/app-v$v" >/dev/null && die "app-v$v already exists: qrtz $v is released (bump: node scripts/app/version.mjs bump)"
rc=0; git ls-remote --exit-code --tags origin "refs/tags/app-v$v" >/dev/null 2>&1 || rc=$?
case "$rc" in
  0) die "app-v$v already exists on origin: qrtz $v is released (bump the version)" ;;
  2) ;;
  *) die "can't reach origin to check for app-v$v" ;;
esac
main="$(git ls-remote origin refs/heads/main | cut -f1)"
[[ -n "$main" ]] || die "can't read origin's main"
head="$(git rev-parse HEAD)"
[[ "$head" == "$main" ]] || git merge-base --is-ancestor "$head" "$main" 2>/dev/null ||
  die "HEAD ${head:0:7} isn't on origin's main (push it first, or git fetch if it is)"
lv="$(live_version)"
if [[ -n "$lv" ]] && ! gt "$v" "$lv"; then
  die "the live feed already has qrtz $lv (>= $v): published objects are immutable, bump the version"
fi
tgzname="qrtz_${v}_aarch64.app.tar.gz"
code="$(http -I -o /dev/null "$host/v1/app/$v/$tgzname")"
case "$code" in
  404) ;;
  200) die "v1/app/$v/$tgzname is already up (an earlier, partial publish?): objects are immutable, bump the version" ;;
  *) die "$host/v1/app/$v/$tgzname: HTTP $code (can't tell what is live; nothing done)" ;;
esac

if [[ -z "$app" ]]; then
  for t in "${CARGO_TARGET_DIR:-}" "$repo/target" "$repo/apps/desktop/src-tauri/target"; do
    [[ -n "$t" && -d "$t/release/bundle/macos/qrtz.app" ]] && { app="$t/release/bundle/macos/qrtz.app"; break; }
  done
  [[ -n "$app" ]] || die "no built qrtz.app (bun run release:mac first, or --app <qrtz.app>)"
fi
app="$(cd "$app" 2>/dev/null && pwd)" || die "--app: no such directory"
[[ "$(basename "$app")" == qrtz.app ]] || die "$app isn't a qrtz.app"
dmg="${dmg:-$(dirname "$app")/qrtz_${v}_aarch64.dmg}"
[[ -s "$dmg" ]] || die "no $dmg (the notarized dmg of qrtz $v; bun run release:mac)"
echo "publish-local: qrtz $v from $app (live feed: ${lv:-none})"

say "the bundle is qrtz $v with echo"
plist=/usr/libexec/PlistBuddy
if [[ $dry == 1 ]]; then
  echo "+ check $app: version $v, ai.qrtz.desktop, qrtz-echo is ai.qrtz.echo by MR26CCZ496, the app notarized by MR26CCZ496"
else
  [[ "$("$plist" -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" == "$v" ]] || die "$app isn't qrtz $v"
  [[ "$("$plist" -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" == ai.qrtz.desktop ]] || die "$app isn't ai.qrtz.desktop"
  [[ -x "$app/Contents/MacOS/qrtz-echo" ]] || die "qrtz-echo isn't bundled (ADR 0174)"
  # Capture first: with pipefail, grep -q exiting early SIGPIPEs codesign and fails the check on a good bundle.
  echo_info="$(codesign -dv "$app/Contents/MacOS/qrtz-echo" 2>&1)" || die "can't read qrtz-echo's signature"
  grep -qx 'Identifier=ai.qrtz.echo' <<<"$echo_info" || die "qrtz-echo isn't ai.qrtz.echo"
  codesign --verify --strict -R='anchor apple generic and identifier "ai.qrtz.echo" and certificate leaf[subject.OU] = "MR26CCZ496"' "$app/Contents/MacOS/qrtz-echo"
  codesign --verify --deep --strict -R='anchor apple generic and identifier "ai.qrtz.desktop" and certificate leaf[subject.OU] = "MR26CCZ496" and notarized' "$app"
fi
# The release gate shared with release:mac and app-release.yml: every helper's launch requirement, entitlements,
# notarization and staple (the 0.2.4 candidate shipped tauri build's signatures and the app refused to start echo).
run bash "$repo/scripts/release/verify-bundle.sh" "$app"

say "the update archive, signed with the app-update key"
if [[ $dry == 0 ]]; then work="$(mktemp -d)"; fi
out="${work:-<work>}"
tgz="$out/$tgzname"
# COPYFILE_DISABLE: no ._ AppleDouble files (they would break the bundle's seal when unpacked).
run env COPYFILE_DISABLE=1 tar -C "$(dirname "$app")" -czf "$tgz" qrtz.app
# The signature's trusted comment carries the version; the app requires it to match the feed's.
with_key --quiet "$tauri" signer sign --app-version "$v" "$tgz"
if [[ $dry == 0 ]]; then
  [[ -s "$tgz.sig" ]] || die "no signature written"
  grep -v '^#' "$pubkey" | tr -d '[:space:]' | base64 --decode >"$out/k.pub"
  base64 --decode <"$tgz.sig" >"$out/k.sig"
fi
# It verifies against the public key the app has compiled in (a key mismatch fails here, not on Macs).
run minisign -V -p "$out/k.pub" -x "$out/k.sig" -m "$tgz"
if [[ $dry == 0 ]]; then
  grep -q "version:$v" "$out/k.sig" || die "the signature's trusted comment doesn't name version:$v"
  rm -f "$out/k.pub" "$out/k.sig"
  bun scripts/release-notes.mjs "$v" >"$out/notes.md" 2>/dev/null || : >"$out/notes.md"
else
  echo "+ bun scripts/release-notes.mjs $v > $out/notes.md"
fi
run_to "$out/feed.json" node scripts/app/feed.mjs --version "$v" --archive "$tgz" --sig "$tgz.sig" \
  --notes-file "$out/notes.md" --expires-days 30
[[ $dry == 1 ]] || cat "$out/feed.json"
with_key bash scripts/app/sign-feed.sh "$out/feed.json" "$out/latest.json"
[[ $dry == 1 ]] || rm -f "$out/feed.json"

say "source archives (qrtz at HEAD, the exact FluidAudio pin, the build scripts)"
run bash "$repo/scripts/release/source-archives.sh" "$v" "$out" HEAD
say "upload to modules.qrtz.ai (R2 v1/app/, latest.json last)"
with_r2 env VERSION="$v" bash scripts/app/upload.sh "$out" "$dmg"
say "what modules.qrtz.ai serves is what was signed"
run env VERSION="$v" QRTZ_APP_HOST="$host" bash scripts/app/verify-live.sh "$out" "$dmg"
if [[ $tag == 1 ]]; then
  say "tag app-v$v"
  run node scripts/app/version.mjs tag --push
else
  echo "publish-local: --no-tag: tag it later with node scripts/app/version.mjs tag --push"
fi
[[ $dry == 1 ]] && echo "publish-local: dry run, nothing done" || echo "publish-local: qrtz $v is published"
