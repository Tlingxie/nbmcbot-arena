#!/bin/sh
set -eu
if [ "$(uname -s)" != Linux ]; then
    echo 'Hard memory containment requires Linux cgroup v2 with a systemd user manager.' >&2
    exit 1
fi
nbmcbot_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ ! -r /sys/fs/cgroup/cgroup.controllers ]; then
    echo 'cgroup v2 is unavailable.' >&2
    exit 1
fi
exec systemd-run --user --scope --quiet --collect \
    --property=MemoryMax=256000000 --property=MemorySwapMax=0 \
    "$nbmcbot_root/target/release/nbmcbot" "$@"
