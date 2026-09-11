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
spike_state=${OPENSHELL_SCRIPTS_TEST_STATE:-$(mktemp -d /private/tmp/lns-os-scripts.XXXXXX)}
export LNS_HOME="$spike_state/home"
export LNS_SOCKET_PATH="$spike_state/lns.sock"
echo "Scripts E2E state: $spike_state"
trap '"$root/target/debug/lns" service stop' EXIT HUP INT TERM
"$root/target/debug/lns" service start
output=$(perl -e 'alarm 180; exec @ARGV or die $!' "$root/target/debug/lns" run --yes --quiet --tty=false --interactive=false -f "$root/spikes/openshell-network/e2e/scripts.yaml" 2>&1) || { printf '%s\n' "$output"; exit 1; }
printf '%s\n' "$output"
printf '%s\n' "$output" | rg -q root-script-confined-ok
printf '%s\n' "$output" | rg -q openshell-pre-start-ok
code=0
output=$(perl -e 'alarm 60; exec @ARGV or die $!' "$root/target/debug/lns" run --yes --quiet --tty=false --interactive=false -f "$root/spikes/openshell-network/e2e/scripts-failure.yaml" 2>&1) || code=$?
printf '%s\n' "$output"
test "$code" = 125
printf '%s\n' "$output" | rg -q 'workload did not start'
if printf '%s\n' "$output" | rg -q 'forbidden-'; then exit 1; fi
echo openshell-script-failure-refused-ok
