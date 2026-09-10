#!/bin/sh
# Rebuilds the committed component. Needs `rustup target add wasm32-wasip2`.
# `--check` rebuilds and reports the committed component if the source no longer
# produces it, so a stale component cannot pass silently.
set -eu
# Part of the contract, not convenience: a panicking component embeds the path
# cargo was given, so building from here keeps that `src/lib.rs` and not this
# machine's.
cd "$(dirname "$0")"
# --locked: the digest of these bytes is what a grant is reserved against, so a
# resolution that drifted must fail loudly rather than rewrite the lock.
cargo build --locked --release --target wasm32-wasip2
built=target/wasm32-wasip2/release/github_mechanism.wasm
if [ "${1:-}" = "--check" ]; then
    # This crate is outside the workspace, so `cargo fmt --all` and
    # `cargo clippy --workspace` never reach it. Nothing else would.
    cargo fmt -- --check
    cargo clippy --locked --release --target wasm32-wasip2 -- -D warnings
    cmp -s "$built" ../sign-in.wasm && exit 0
    echo "connectors/github/sign-in.wasm is stale" >&2
    echo "run connectors/github/mechanism/build.sh to rebuild it" >&2
    exit 1
fi
cp "$built" ../sign-in.wasm
