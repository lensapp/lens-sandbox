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

# What each build script remaps its registry path onto, so a host that already
# keeps its registry there is not told its clean bytes are a leak it can never
# rebuild away.
REMAPPED_TO=/cargo

# A component that embeds a home directory is bytes only this machine rebuilds,
# so `cmp` agrees here and on no other host. The toolchain's own `/rustc/<hash>`
# paths are identical everywhere the pin runs, so they stay.
#
# A scan that read nothing is reported rather than passed: components that moved
# out from under it would leave it green and dead.
embeds_a_home_directory() {
    set -- "$1"/*.wasm
    [ -e "$1" ] || return 2
    found=0
    under_a_home=$(LC_ALL=C grep -alE '/(root|home|Users)/' "$@") || found=$?
    [ "$found" -le 1 ] || return 3
    # Where this machine keeps its registry, which a host that keeps it outside a home directory would otherwise leak past the pattern above.
    this_cargo=${CARGO_HOME:-$HOME/.cargo}
    this_cargo=${this_cargo%/}
    under_this_cargo=""
    if [ "$this_cargo" != "${REMAPPED_TO%/}" ]; then
        under_this_cargo=$(LC_ALL=C grep -alF "$this_cargo" "$@") || found=$?
        [ "$found" -le 1 ] || return 3
    fi
    printf '%s\n%s\n' "$under_a_home" "$under_this_cargo" | grep -v '^$' | sort -u
}

status=0
failed=""
for script in "$@"; do
    if ! "$script" --check; then
        status=1
        failed="$failed $script"
        continue
    fi
    beside=$(cd "$(dirname "$script")/.." && pwd)
    scanned=0
    leaked=$(embeds_a_home_directory "$beside") || scanned=$?
    if [ "$scanned" -eq 2 ]; then
        echo "components: $beside holds no component for $script to be checked against" >&2
    elif [ "$scanned" -ne 0 ]; then
        echo "components: the components in $beside could not be read" >&2
    elif [ -n "$leaked" ]; then
        echo "these components embed the machine that built them:" >&2
        echo "$leaked" >&2
        echo "run $script to rebuild them" >&2
    else
        continue
    fi
    status=1
    failed="$failed $script"
done

if [ "$status" -ne 0 ]; then
    echo "components failed:$failed" >&2
fi
exit "$status"
