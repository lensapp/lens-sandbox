#!/bin/sh
# Rebuilds the committed components, one per method. Needs
# `rustup target add wasm32-wasip2`. `--check` rebuilds and reports any
# committed component the source no longer produces, so a stale one cannot pass
# silently.
set -eu
# Part of the contract, not convenience: a panicking component embeds the path
# cargo was given, so building from here keeps that `src/lib.rs` and not this
# machine's.
cd "$(dirname "$0")"
# Set, never appended to: an inherited RUSTFLAGS is host state, and these
# bytes must carry none — without the remap a dependency embeds this
# machine's registry path.
export RUSTFLAGS="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"
check=${1:-}
if [ "$check" = "--check" ]; then
    # This crate is outside the workspace, so `cargo fmt --all` and
    # `cargo clippy --workspace` never reach it. Nothing else would.
    cargo fmt -- --check
fi
# A rebuild that disagrees says how, because "run build.sh" is no help to the
# host whose rebuild is the thing disagreeing.
say_how_it_differs() {
    export LC_ALL=C
    echo "  rebuilt by $(rustc --version) on $(rustc -vV | grep '^host: ')" >&2
    echo "  $2: $(wc -c < "$2") bytes committed, $(wc -c < "$1") rebuilt" >&2
    cmp "$2" "$1" 2>&1 | head -1 >&2
    work=$(mktemp -d) || return 0
    tr -cs '[:print:]' '\n' < "$2" | sort -u > "$work/committed"
    tr -cs '[:print:]' '\n' < "$1" | sort -u > "$work/rebuilt"
    diff -u "$work/committed" "$work/rebuilt" > "$work/diff" || true
    echo "  $(grep -c '^[-+][^-+]' "$work/diff") differing strings, first 40 lines:" >&2
    head -40 "$work/diff" >&2
    rm -rf "$work"
}

stale=""
for variant in sign-in oauth-sign-in; do
    # --locked: the digest of these bytes is what a grant is reserved against,
    # so a resolution that drifted must fail loudly rather than rewrite the lock.
    cargo build --locked --release --target wasm32-wasip2 \
        --no-default-features --features "$variant"
    built=target/wasm32-wasip2/release/github_mechanism.wasm
    if [ "$check" = "--check" ]; then
        # Inside the loop: each variant compiles code the other one does not.
        cargo clippy --locked --release --target wasm32-wasip2 \
            --no-default-features --features "$variant" -- -D warnings
        cmp -s "$built" "../$variant.wasm" || {
            stale="$stale $variant"
            say_how_it_differs "$built" "../$variant.wasm"
        }
    else
        cp "$built" "../$variant.wasm"
    fi
done
if [ -n "$stale" ]; then
    echo "stale committed components:$stale" >&2
    echo "run connectors/github/mechanism/build.sh to rebuild them" >&2
    exit 1
fi
