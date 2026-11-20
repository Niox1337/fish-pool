#!/usr/bin/env bash
# Compiles fish-node for linux/amd64 on this machine, uploads it with the sidecar and deploy files,
# then rebuilds the runtime image on the node host and restarts the node container.
#
# Environment:
#   FISH_SSH      ssh destination, default root@<SSH from .env>
#   FISH_SSH_KEY  private key, default ~/.ssh/id_ed25519_fish
#   RUST_IMAGE    Rust toolchain image for the local compile, for example mirror.gcr.io/library/rust:1.94-bookworm
#   FISH_REMOTE   repository path on the host, default /opt/fish
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [ "$#" -ne 1 ] || { [ "$1" != --build ] && [ "$1" != --deploy ]; }; then
  echo "Usage: $0 --build | --deploy" >&2
  exit 2
fi

HOST_FROM_ENV="$(grep '^SSH=' .env 2>/dev/null | cut -d= -f2- || true)"
DEST="${FISH_SSH:-root@$HOST_FROM_ENV}"
KEY="${FISH_SSH_KEY:-$HOME/.ssh/id_ed25519_fish}"
REMOTE="${FISH_REMOTE:-/opt/fish}"
SSH=(ssh -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes "$DEST")

echo "==> compiling fish-node for linux/amd64"
docker buildx build --platform linux/amd64 -f deploy/node.Dockerfile \
  ${RUST_IMAGE:+--build-arg "RUST_IMAGE=$RUST_IMAGE"} \
  --target bin-export --output type=local,dest=deploy/dist .
file deploy/dist/fish-node 2>/dev/null || ls -l deploy/dist/fish-node
[ "$1" = --build ] && exit 0

if [ -z "$HOST_FROM_ENV" ] && [ -z "${FISH_SSH:-}" ]; then
  echo "Set FISH_SSH or SSH in .env" >&2
  exit 1
fi

echo "==> uploading to $REMOTE"
LOCK="$REMOTE/.deploy.lock"
"${SSH[@]}" "flock -n $LOCK true" \
  || { echo "Another deploy holds $LOCK on the host" >&2; exit 1; }
COPYFILE_DISABLE=1 XZ_OPT=-9e tar cJf - --exclude __pycache__ --exclude '*.pyc' \
  deploy/node.Dockerfile deploy/node.Dockerfile.dockerignore deploy/compose.yml deploy/node-entrypoint.sh \
  deploy/dist/fish-node sidecar/pyproject.toml sidecar/uv.lock sidecar/fish_sidecar node/Cargo.toml node/Cargo.lock \
  node/rustfmt.toml node/src \
  | "${SSH[@]}" "tar xJf - -C $REMOTE"

echo "==> building runtime image and restarting"
"${SSH[@]}" "flock -n $LOCK sh -c '
  cd $REMOTE && { grep -q ^NODE_BIN= deploy/.env || echo NODE_BIN=prebuilt >> deploy/.env; }
  docker compose -f deploy/compose.yml build --progress plain node > build.log 2>&1; code=\$?; echo BUILD_EXIT=\$code >> build.log
  if [ \$code -ne 0 ]; then tail -30 build.log; exit \$code; fi
  docker compose -f deploy/compose.yml up -d --no-build
  docker image prune -f >/dev/null
  docker builder prune -f --max-used-space 512MB >/dev/null 2>&1 || docker builder prune -f --keep-storage 512MB >/dev/null
  docker compose -f deploy/compose.yml ps'"
