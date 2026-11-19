#!/usr/bin/env bash
# Runs the console, node and sidecar against local external-service fixtures.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export TMPDIR=/private/tmp
DRIVER_PID=""
cleanup() {
  if [ -n "$DRIVER_PID" ]; then
    kill -TERM "$DRIVER_PID" 2>/dev/null || true
    wait "$DRIVER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
case "${1:-}" in
  ""|--keep) ;;
  *) echo 'usage: scripts/e2e.sh [--keep]' >&2; exit 2 ;;
esac
for tool in cargo worker-build wrangler uv python3; do
  command -v "$tool" >/dev/null || { echo "FAIL missing $tool" >&2; exit 1; }
done
printf 'building node\n'
(cd "$ROOT/node" && CC=cc CXX=c++ cargo build --offline)
printf 'building console\n'
(cd "$ROOT/console" && CC=clang worker-build --release)
python3 -u "$ROOT/scripts/e2e/run.py" "$@" &
DRIVER_PID=$!
wait "$DRIVER_PID"
DRIVER_PID=""
