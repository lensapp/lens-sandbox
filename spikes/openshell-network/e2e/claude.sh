#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
export LNS_SERVICE_BIN="$root/target/debug/lns-service"
export LNS_INIT_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-init"
export LNS_SESSION_BROKER_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-session-broker"
export LNS_SUPERVISOR_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-supervisor"
export LNS_NFT_BIN="$root/vendor/static-nft/nft-1.1.5-linux-arm64-musl"
export LNS_HEADLESS=1
for artifact in "$LNS_SERVICE_BIN" "$LNS_INIT_BIN" "$LNS_SESSION_BROKER_BIN" "$LNS_SUPERVISOR_BIN" "$LNS_NFT_BIN"; do
    test -s "$artifact" || { echo "Missing artifact: $artifact" >&2; exit 1; }
done
spike_state=${OPENSHELL_CLAUDE_TEST_STATE:-$(mktemp -d /private/tmp/lns-os-claude.XXXXXX)}
export LNS_HOME="$spike_state/home"
export LNS_SOCKET_PATH="$spike_state/lns.sock"
echo "Claude install E2E state: $spike_state"
trap '"$root/target/debug/lns" service stop' EXIT HUP INT TERM
"$root/target/debug/lns" service start
perl -e 'alarm 300; exec @ARGV or die $!' "$root/target/debug/lns" run --yes --tty=false --interactive=false -f "$root/spikes/openshell-network/e2e/claude.yaml"
