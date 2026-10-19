#!/usr/bin/env bash
# Source this: an rclone remote `qrtzr2:` for the R2 bucket from R2_ACCOUNT_ID, R2_ACCESS_KEY_ID and
# R2_SECRET_ACCESS_KEY (the same RCLONE_CONFIG_* variables publish.sh sets; no rclone.conf with keys in it).
# The loop variable is r2_env_var, not v: callers (scripts/app/upload.sh) keep the version in $v.
for r2_env_var in R2_ACCOUNT_ID R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY; do
  [[ -n "${!r2_env_var:-}" ]] || { echo "r2-env: $r2_env_var is not set" >&2; return 1 2>/dev/null || exit 1; }
done
unset r2_env_var
export RCLONE_CONFIG_QRTZR2_TYPE=s3 RCLONE_CONFIG_QRTZR2_PROVIDER=Cloudflare
export RCLONE_CONFIG_QRTZR2_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" RCLONE_CONFIG_QRTZR2_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
export RCLONE_CONFIG_QRTZR2_ENDPOINT="https://$R2_ACCOUNT_ID.r2.cloudflarestorage.com"
export RCLONE_CONFIG_QRTZR2_NO_CHECK_BUCKET=true RCLONE_CONFIG_QRTZR2_ACL=private
