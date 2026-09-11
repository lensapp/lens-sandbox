#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
export LNS_SERVICE_BIN="$root/target/debug/lns-service"
export LNS_INIT_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-init"
export LNS_SESSION_BROKER_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-session-broker"
export LNS_SUPERVISOR_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-supervisor"
export LNS_NFT_BIN="$root/vendor/static-nft/nft-1.1.5-linux-arm64-musl"
export LNS_HOME="$root/target/openshell-manual"
export LNS_SOCKET_PATH="/private/tmp/lns-openshell-$(id -u)/lns.sock"
exec "$root/target/debug/lns" "$@"
