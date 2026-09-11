#!/usr/bin/env bash
# Smoke test for the Linux per-run passt network backend.
#
# Locks in the four user-facing contracts of the Linux netdev change:
#
#   1. A guest reaches the internet through the in-guest proxy, over a
#      virtio-net device cloud-hypervisor connects to the run's own
#      passt across a vhost-user socket.
#   2. The guest's address comes from passt's DHCP server, on the
#      private subnet lns asks for rather than the host's own address.
#   3. The guest's resolver is the gateway passt answers, 192.168.127.1.
#   4. The run's audit chain names the backend that served the link.
#
# Run with `bash crates/lns-cli/tests/smoke/passt-netdev.sh` from the
# workspace root. NOT in the pre-merge gate — it needs a live microVM,
# and no GitHub-hosted Linux runner can boot one (no nested KVM). Run it
# by hand on a KVM host. It skips itself with exit 0 wherever passt or
# /dev/kvm is missing.

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

[ "$(uname -s)" = "Linux" ] || skip "passt is the Linux backend; this host is $(uname -s)."
[ -r /dev/kvm ] && [ -w /dev/kvm ] || skip "no usable /dev/kvm; a guest cannot boot here."
[ -x "$LNS" ] || skip "no lns binary at ${LNS}; build one with \`make build-lns\`."

if [ -z "${LNS_PASST_BIN:-}" ] && ! command -v passt >/dev/null 2>&1; then
  FOUND=""
  for dir in /usr/bin /usr/local/bin /usr/sbin /usr/local/sbin; do
    [ -x "$dir/passt" ] && FOUND="$dir/passt"
  done
  [ -n "$FOUND" ] || skip "no passt on this host; \`apt install passt\` or set LNS_PASST_BIN."
fi

"$LNS" service status >/dev/null 2>&1 || skip "the lns service is not running; start it with \`lns service start\`."

RUN_NAME="passt-smoke-$$"

printf '[test]  a guest reaches a host through the guest proxy\n'
OUTPUT=$("$LNS" run --rm --name "$RUN_NAME" -- \
  sh -c 'curl -sS -o /dev/null -w "%{http_code}" https://example.com; echo; ip -4 addr show eth0; cat /etc/resolv.conf') \
  || fail "the run did not complete; the guest has no working network."

case "$OUTPUT" in
  200*) pass "the guest reached example.com through the guest proxy" ;;
  *) fail "expected a 200 from example.com, got: ${OUTPUT}" ;;
esac

case "$OUTPUT" in
  *"inet 192.168.127.2"*) pass "the guest address came from the run's passt" ;;
  *) fail "expected the passt-served guest address on eth0, got: ${OUTPUT}" ;;
esac

case "$OUTPUT" in
  *"nameserver 192.168.127.1"*) pass "the guest resolver is the passt gateway" ;;
  *) fail "expected the passt gateway as the guest resolver, got: ${OUTPUT}" ;;
esac

printf '[test]  the audit chain names the backend that served the link\n'
AUDIT=$("$LNS" audit) || fail "\`lns audit\` failed."
case "$AUDIT" in
  *"guest network served by passt"*) pass "the chain names passt" ;;
  *) fail "expected a passt network line in \`lns audit\`." ;;
esac

printf '[ok]    the guest network came from its own passt\n'
