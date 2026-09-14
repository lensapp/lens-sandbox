#!/bin/sh
set -eu
cd "$(dirname "$0")"
export RUSTFLAGS="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$(rustc --print sysroot)=/rust-toolchain"
cargo build --locked --release --target wasm32-wasip2
built=target/wasm32-wasip2/release/linear_mechanism.wasm
if [ "${1:-}" = "--check" ]; then
    cargo fmt -- --check
    cargo clippy --locked --release --target wasm32-wasip2 -- -D warnings
    cmp "$built" ../sign-in.wasm
else
    cp "$built" ../sign-in.wasm
fi
