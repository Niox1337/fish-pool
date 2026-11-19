#!/usr/bin/env bash
# Inserts one invite code directly into D1 and prints it. Used to bootstrap the service
# before any admin exists, and from scripts.
#
# Usage: scripts/make-invite.sh <days> [--remote] [--max-uses N] [--note TEXT] [extra wrangler args]
#   --remote  run against the deployed D1 database (default is the local dev database)
#   extra args are passed to `wrangler d1 execute`, for example --config or --persist-to
# The code is printed alone on the last line of output.

set -euo pipefail

CONSOLE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WRANGLER="${WRANGLER:-wrangler}"

if [ $# -lt 1 ]; then
  echo "usage: $0 <days> [--remote] [--max-uses N] [--note TEXT] [extra wrangler args]" >&2
  exit 2
fi
DAYS="$1"
shift
if ! [[ "$DAYS" =~ ^[0-9]+$ ]] || [ "$DAYS" -lt 1 ] || [ "$DAYS" -gt 3650 ]; then
  echo "days must be an integer from 1 to 3650" >&2
  exit 2
fi

MODE="--local"
MAX_USES=1
NOTE="make-invite.sh"
EXTRA=()
while [ $# -gt 0 ]; do
  case "$1" in
    --remote) MODE="--remote" ;;
    --local) MODE="--local" ;;
    --max-uses) MAX_USES="${2:-}"; shift ;;
    --note) NOTE="${2:-}"; shift ;;
    *) EXTRA+=("$1") ;;
  esac
  shift
done
if ! [[ "$MAX_USES" =~ ^[0-9]+$ ]] || [ "$MAX_USES" -lt 1 ] || [ "$MAX_USES" -gt 1000 ]; then
  echo "--max-uses must be an integer from 1 to 1000" >&2
  exit 2
fi
NOTE="${NOTE//\'/\'\'}"

# 8 random bytes, each mapped to one Crockford base32 symbol. 256 is a multiple of 32,
# so byte % 32 is uniform.
ALPHABET="0123456789ABCDEFGHJKMNPQRSTVWXYZ"
SYMBOLS=""
for b in $(od -An -N8 -tu1 /dev/urandom); do
  SYMBOLS="$SYMBOLS${ALPHABET:$((b % 32)):1}"
done
CODE="FISH-${SYMBOLS:0:4}-${SYMBOLS:4:4}"
NOW_MS="$(( $(date +%s) * 1000 ))"

cd "$CONSOLE_DIR"
"$WRANGLER" d1 execute DB "$MODE" ${EXTRA[@]+"${EXTRA[@]}"} --command \
  "INSERT INTO invite_codes (code, days, max_uses, used_count, note, created_by, created_at) VALUES ('$CODE', $DAYS, $MAX_USES, 0, '$NOTE', NULL, $NOW_MS);" >&2
echo "$CODE"
