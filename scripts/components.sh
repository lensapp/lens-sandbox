#!/bin/sh
# Runs each committed component's build script with `--check`.
#
# USAGE: components.sh <build.sh> [<build.sh> ...]
#
# Every script runs even after one fails, and the worst status is the one
# reported: a caller that stopped at the first failure would name one stale
# component and hide the rest, and a caller that took the last status would
# hide every failure but the last.
#
# It needs the wasm32-wasip2 target, which a Rust-only checkout need not have,
# so it prints what it skipped rather than blocking a commit — and in CI, where
# the lint job installs the target, a skip fails, because silence must not read
# as agreement.
set -eu

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <build.sh> [<build.sh> ...]" >&2
    exit 2
fi

if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-wasip2; then
    echo "components note: skipped (rustup target add wasm32-wasip2 to run it)"
    [ -z "${CI:-}" ] || {
        echo "components: CI installs the target, so a skip here is a dead step" >&2
        exit 1
    }
    exit 0
fi

status=0
failed=""
for script in "$@"; do
    "$script" --check || {
        status=1
        failed="$failed $script"
    }
done

if [ "$status" -ne 0 ]; then
    echo "components failed:$failed" >&2
fi
exit "$status"
