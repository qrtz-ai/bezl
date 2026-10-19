#!/usr/bin/env bash
# ADR 0181: put one qrtz.app release on modules.qrtz.ai (R2 bucket qrtz-modules, under v1/app/). app-release.yml runs it.
#
#   scripts/app/upload.sh <dir with qrtz_<v>_aarch64.app.tar.gz{,.sig} and latest.json> <notarized dmg>
#
# Order: the immutable versioned files first (v1/app/<v>/: the archive, its signature, the dmg; rclone --immutable
# refuses to change one that is already up with other bytes), then the stable dmg (v1/app/qrtz-latest.dmg) and last
# the feed (v1/app/latest.json), both cached 60 s, so no feed ever names an archive that isn't up yet.
# Needs R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY (scripts/ci/r2-env.sh) and VERSION.
set -euo pipefail
dir="${1:?usage: upload.sh <dir> <dmg>}"
dmg="${2:?usage: upload.sh <dir> <dmg>}"
v="${VERSION:?VERSION is not set}"
[[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "upload: not a plain x.y.z version: $v" >&2; exit 1; }
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
. "$repo/scripts/ci/r2-env.sh"
bucket="qrtzr2:${R2_BUCKET:-qrtz-modules}/v1/app"
tgz="$dir/qrtz_${v}_aarch64.app.tar.gz"
for f in "$tgz" "$tgz.sig" "$dir/latest.json" "$dmg"; do [[ -s "$f" ]] || { echo "upload: missing $f" >&2; exit 1; }; done
grep -q "\"version\": \"$v\"" "$dir/latest.json" || { echo "upload: latest.json isn't for $v" >&2; exit 1; }

forever=(--immutable --checksum --header-upload "Cache-Control: public, max-age=31536000, immutable")
short=(--ignore-times --header-upload "Cache-Control: public, max-age=60")
rclone copyto "$tgz" "$bucket/$v/$(basename "$tgz")" "${forever[@]}" -v
rclone copyto "$tgz.sig" "$bucket/$v/$(basename "$tgz").sig" "${forever[@]}" -v
rclone copyto "$dmg" "$bucket/$v/qrtz_${v}_aarch64.dmg" "${forever[@]}" -v
rclone copyto "$dmg" "$bucket/qrtz-latest.dmg" "${short[@]}" -v
rclone copyto "$dir/latest.json" "$bucket/latest.json" "${short[@]}" --header-upload "Content-Type: application/json" -v
echo "upload: qrtz $v is up (v1/app/$v/, qrtz-latest.dmg, latest.json)"
