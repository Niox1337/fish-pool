#!/bin/sh
# Makes the container cgroup tree writable when possible, then starts the node.
set -e
if [ -d /sys/fs/cgroup ] && [ ! -w /sys/fs/cgroup/cgroup.procs ]; then
    mount -o remount,rw /sys/fs/cgroup 2>/dev/null || echo "cgroup tree is read only, per tenant memory limits disabled" >&2
fi
mkdir -p "${FISH_DATA_DIR:-/data}"
exec /usr/local/bin/fish-node "$@"
