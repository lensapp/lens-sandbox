#!/bin/sh
set -eu

spike_manifest="$(dirname "$0")/Cargo.toml"
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
git -C "$root/target/openshell" diff --exit-code -- crates/openshell-supervisor-process
cargo test --locked --manifest-path "$root/Cargo.toml" -p lns-session-broker protect
cargo test --locked --manifest-path "$root/Cargo.toml" -p lns-supervisor --all-targets
cargo fmt --manifest-path "$spike_manifest" -- --check
cargo clippy --locked --manifest-path "$spike_manifest" --all-targets --all-features -- -D warnings -D clippy::cognitive_complexity
OPENSHELL_NETWORK_BINARY_IDENTITY=endpoint-only cargo test --locked --manifest-path "$spike_manifest" --all-features
