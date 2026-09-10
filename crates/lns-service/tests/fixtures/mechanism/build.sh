#!/bin/sh
# Rebuilds the committed components. Needs `rustup target add wasm32-wasip2`.
# `--check` rebuilds and reports any committed component the source no longer
# produces, so a stale fixture cannot pass silently. Unlike
# connectors/github/mechanism/build.sh it does not yet run fmt or clippy over
# this out-of-workspace crate, so nothing lints it; adding them means
# reformatting the source and recommitting all twenty components, because a
# trapping fixture embeds its own file and line.
set -eu
# Part of the contract, not convenience: a panicking fixture embeds the path cargo
# was given, so building from here keeps that `src/lib.rs` and not this machine's.
cd "$(dirname "$0")"
check=${1:-}
stale=""
for name in fetching running asking hanging expiring trapping prying hoarding showing shouting forging labelling straying granting failing picking hurrying refusing keeping clinging; do
    # --locked: a gate compares these bytes, so a resolution that drifted must fail loudly rather than rewrite the lock and change what is built.
    cargo build --locked --release --target wasm32-wasip2 --no-default-features --features "$name"
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
