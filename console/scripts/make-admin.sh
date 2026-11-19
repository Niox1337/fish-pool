#!/usr/bin/env bash
# Gives an existing account the admin role directly in D1.
#
# Usage: scripts/make-admin.sh <email> [--remote] [extra wrangler args]
#   --remote  run against the deployed D1 database (default is the local dev database)
#   extra args are passed to `wrangler d1 execute`, for example --config or --persist-to

set -euo pipefail

CONSOLE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WRANGLER="${WRANGLER:-wrangler}"

if [ $# -lt 1 ]; then
  echo "usage: $0 <email> [--remote] [extra wrangler args]" >&2
  exit 2
fi
EMAIL="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
shift
if ! printf '%s' "$EMAIL" | grep -Eq "^[^@[:space:]'\"\\\\]+@[^@[:space:]'\"\\\\]+\.[^@[:space:]'\"\\\\]+$"; then
  echo "invalid email: $EMAIL" >&2
  exit 2
fi

MODE="--local"
EXTRA=()
for a in "$@"; do
  case "$a" in
    --remote) MODE="--remote" ;;
    --local) MODE="--local" ;;
    *) EXTRA+=("$a") ;;
  esac
done

cd "$CONSOLE_DIR"
"$WRANGLER" d1 execute DB "$MODE" ${EXTRA[@]+"${EXTRA[@]}"} --command \
  "UPDATE users SET role = 'admin' WHERE email = '$EMAIL'; SELECT id, email, role FROM users WHERE email = '$EMAIL';"
echo "done. If the result above is empty, no account with $EMAIL exists yet: register first or add it to ADMIN_EMAILS."
