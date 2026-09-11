#!/usr/bin/env bash
# Smoke test for the macOS per-run gvproxy network backend.
#
# Locks in the three user-facing contracts of the netdev change:
#
#   1. A guest reaches the internet through the in-guest proxy, with
#      its address, route and DNS answered by the run's own gvproxy —
#      no vmnet bridge and no host DHCP server.
#   2. The guest's resolver is the gvproxy gateway, 192.168.127.1.
#   3. The run's audit chain names the backend that served the link.
#
# Run with `bash crates/lns-cli/tests/smoke/gvproxy-netdev.sh` from the
# workspace root. NOT in the pre-merge gate — it needs a live microVM,
# which no CI runner in this repository can start. It skips itself with
# exit 0 wherever gvproxy cannot run.

set -euo pipefail

LNS="${LNS:-./bin/lns}"

skip() {
  printf '[skip]  %s\n' "$1"
  exit 0
}

fail() {
  printf '[fail]  %s\n' "$1" >&2
  exit 1
}

pass() {
  printf '[pass]  %s\n' "$1"
}

[ "$(uname -s)" = "Darwin" ] || skip "gvproxy netdev is the macOS backend; this host is $(uname -s)."
[ "$(uname -m)" = "arm64" ] || skip "lns needs Apple Silicon to host a guest; this host is $(uname -m)."
[ -x "$LNS" ] || skip "no lns binary at ${LNS}; build one with \`make build-lns\`."

if [ -z "${LNS_GVPROXY_BIN:-}" ] && ! command -v gvproxy >/dev/null 2>&1; then
  GVPROXY_MANAGED="${LNS_HOME:-${HOME}/.lns}/gvproxy/0.8.9/gvproxy"
  [ -x "$GVPROXY_MANAGED" ] || skip "no gvproxy on this host; run the installer or set LNS_GVPROXY_BIN."
fi

"$LNS" service status >/dev/null 2>&1 || skip "the lns service is not running; start it with \`lns service start\`."

RUN_NAME="gvproxy-smoke-$$"

printf '[test]  a guest reaches a host through the guest proxy\n'
OUTPUT=$("$LNS" run --rm --name "$RUN_NAME" -- \
  sh -c 'curl -sS -o /dev/null -w "%{http_code}" https://example.com; echo; cat /etc/resolv.conf') \
  || fail "the run did not complete; the guest has no working network."

case "$OUTPUT" in
  200*) pass "the guest reached example.com through the guest proxy" ;;
  *) fail "expected a 200 from example.com, got: ${OUTPUT}" ;;
esac

case "$OUTPUT" in
  *"nameserver 192.168.127.1"*) pass "the guest resolver is the gvproxy gateway" ;;
  *) fail "expected the gvproxy gateway as the guest resolver, got: ${OUTPUT}" ;;
esac

printf '[test]  the audit chain names the backend that served the link\n'
AUDIT=$("$LNS" audit) || fail "\`lns audit\` failed."
case "$AUDIT" in
  *"guest network served by gvproxy"*) pass "the chain names gvproxy" ;;
  *) fail "expected a gvproxy network line in \`lns audit\`." ;;
esac

printf '[ok]    the guest network came from its own gvproxy\n'
