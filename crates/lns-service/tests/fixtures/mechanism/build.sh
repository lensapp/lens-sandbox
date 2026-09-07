#!/bin/sh
# Rebuilds the committed components. Needs `rustup target add wasm32-wasip2`.
# `--check` rebuilds and reports any committed component the source no longer
# produces, so a stale fixture cannot pass silently.
set -eu
cd "$(dirname "$0")"
check=${1:-}
stale=""
for name in fetching running asking hanging expiring trapping binding prying hoarding showing shouting forging labelling straying granting failing picking hurrying refusing keeping clinging; do
    cargo build --release --target wasm32-wasip2 --no-default-features --features "$name"
    built=target/wasm32-wasip2/release/mechanism_fixture.wasm
    if [ "$check" = "--check" ]; then
        cmp -s "$built" "../$name.wasm" || stale="$stale $name"
    else
        cp "$built" "../$name.wasm"
    fi
done
if [ -n "$stale" ]; then
    echo "stale committed components:$stale" >&2
    echo "run ./build.sh to rebuild them" >&2
    exit 1
fi
