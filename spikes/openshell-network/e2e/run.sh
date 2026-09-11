#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
cd "$root"
export LNS_SERVICE_BIN="$root/target/debug/lns-service"
export LNS_INIT_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-init"
export LNS_SESSION_BROKER_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-session-broker"
export LNS_SUPERVISOR_BIN="$root/target/aarch64-unknown-linux-musl/debug/lns-supervisor"
export LNS_NFT_BIN="$root/vendor/static-nft/nft-1.1.5-linux-arm64-musl"
export LNS_HEADLESS=1
for artifact in "$LNS_SERVICE_BIN" "$LNS_INIT_BIN" "$LNS_SESSION_BROKER_BIN" "$LNS_SUPERVISOR_BIN" "$LNS_NFT_BIN"; do
    test -s "$artifact" || { echo "Missing artifact: $artifact" >&2; exit 1; }
done
if cargo tree --locked --offline -p lns-supervisor --prefix none | rg '^lens-sandbox-core '; then
    echo 'Old supervisor dependency is still present' >&2
    exit 1
fi
spike_state=$(mktemp -d /private/tmp/lns-os-e2e.XXXXXX)
export LNS_HOME="$spike_state/home"
export LNS_SOCKET_PATH="$spike_state/lns.sock"
echo "E2E state and logs: $spike_state"
trap '"$root/target/debug/lns" service stop' EXIT HUP INT TERM
"$root/target/debug/lns" service start
perl -e 'alarm 120; exec @ARGV or die $!' "$root/target/debug/lns" run --yes --quiet --tty=false --interactive=false -f "$root/spikes/openshell-network/e2e/lns.yaml" -- /bin/sh -c '
    test "$(id -u)" = 65534 || exit 40
    test -z "${LENS_SANDBOX_TOKEN+x}" || exit 41
    test -z "${LENS_SANDBOX_WS_URL+x}" || exit 42
    test "$http_proxy" = http://127.0.0.1:3128 || exit 43
    if wget -T 5 -q -O /dev/null http://1.1.1.1/; then exit 44; fi
    if env -u http_proxy -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY wget -T 5 -q -O /dev/null http://1.1.1.1/; then exit 45; fi
    echo openshell-guest-confinement-ok
'
perl -e 'alarm 120; exec @ARGV or die $!' "$root/target/debug/lns" run --yes --quiet --detach --name openshell-approval -f "$root/spikes/openshell-network/e2e/ask.yaml" -- /bin/sh -c 'wget -T 45 -q -O /tmp/response http://example.com/ && grep -q "Example Domain" /tmp/response && echo openshell-guest-approval-ok; sleep 3'
entry=
attempt=0
while [ -z "$entry" ] && [ "$attempt" -lt 30 ]; do
    entry=$("$root/target/debug/lns" approval ls openshell-approval --format json | jq -r '.[] | select(.answer == "undecided") | .id' | head -1)
    attempt=$((attempt + 1))
    sleep 1
done
test -n "$entry" || { echo 'No pending approval appeared' >&2; exit 1; }
"$root/target/debug/lns" approval answer "$entry" always-allow
result=$(perl -e 'alarm 60; exec @ARGV or die $!' "$root/target/debug/lns" logs -f openshell-approval)
printf '%s\n' "$result"
printf '%s\n' "$result" | rg -q openshell-guest-approval-ok
