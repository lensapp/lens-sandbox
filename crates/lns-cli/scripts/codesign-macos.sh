#!/usr/bin/env bash
# Ad-hoc sign a built `lns` binary with the
# `com.apple.security.virtualization` entitlement, which is required on
# Apple Silicon to call into Vz from a non-AppStore binary.
#
# Usage:
#   crates/lns-cli/scripts/codesign-macos.sh                  # signs target/release/lns
#   crates/lns-cli/scripts/codesign-macos.sh path/to/binary
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "codesign-macos.sh is only relevant on macOS hosts" >&2
    exit 0
fi

here="$(cd "$(dirname "$0")" && pwd)"
crate_dir="$(dirname "$here")"
ent="$crate_dir/lns.entitlements"

if [[ ! -f "$ent" ]]; then
    echo "missing entitlements at $ent" >&2
    exit 1
fi

target="${1:-}"
if [[ -z "$target" ]]; then
    # default: workspace target/release/lns
    repo_root="$(cd "$crate_dir/../.." && pwd)"
    for cand in "$repo_root/target/release/lns" "$repo_root/target/debug/lns"; do
        if [[ -x "$cand" ]]; then
            target="$cand"
            break
        fi
    done
fi

if [[ -z "$target" || ! -x "$target" ]]; then
    echo "no lns binary found; build first (cargo build -p lns [--release])" >&2
    exit 1
fi

codesign --force --sign - --entitlements "$ent" "$target"
echo "signed: $target"
codesign --display --entitlements - "$target" 2>&1 | sed 's/^/  /'
