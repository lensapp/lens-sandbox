#!/usr/bin/env bash
# Smoke test for the macOS in-process netstack network backend.
#
# Locks in the user-facing contracts of the netdev change:
#
#   1. A guest reaches the internet through the in-guest proxy, with
#      its address, route and DNS answered by the run's own netstack —
#      no vmnet bridge and no host DHCP server.
#   2. The guest's resolver is the netstack gateway, 192.168.127.1.
#   3. The run's audit chain names the backend that served the link.
#   4. Nothing else on the guest's subnet answers: neither the gateway
#      on any port but DNS, nor any other address in 192.168.127.0/24.
#   5. A host listener that logs every accept sees none of them. A failed
#      connect alone does not prove nothing was reached.
#
# The gateway also serves DNS over TCP on 192.168.127.1:53, and there is no
# probe for it here. Every unmarked TCP connection the workload makes is
# redirected to the in-guest transparent proxy by the sandbox's own nftables
# rules, so nothing started from this image reaches the gateway on TCP. The
# Rust tests in crates/lns-service/src/vm/netdev/engine.rs drive that path
# through the fake guest instead.
#
# A refused destination shows up in the guest as `HTTP/1.1 502 Bad Gateway`
# with an empty body: the in-guest transparent proxy answers for an upstream
# connect that the netstack reset. curl exits 0 on that 502, so every probe
# below judges the answer by its body — the witness serves `REACHED!` and the
# gateway must serve nothing at all — never by curl's exit status.
#
# Like `interactive-shell.exp`, it writes its own sandbox definition
# into a temp project first. The negative checks need a definition
# whose egress is `match: "*"`, because that is the shape in which a
# reachable gateway service would be a real hole.
#
# The image must carry a real curl. busybox `wget` over HTTPS fails
# through the guest proxy on every backend, so it proves nothing here.
#
# Run with `bash crates/lns-cli/tests/smoke/netstack-netdev.sh` from the
# workspace root. NOT in the pre-merge gate — it needs a live microVM,
# which no CI runner in this repository can start.

set -euo pipefail

LNS="${LNS:-./bin/lns}"
IMAGE="${LNS_SMOKE_IMAGE:-docker.io/curlimages/curl:8.11.1}"

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

[ "$(uname -s)" = "Darwin" ] || skip "the netstack netdev is the macOS backend; this host is $(uname -s)."
[ "$(uname -m)" = "arm64" ] || skip "lns needs Apple Silicon to host a guest; this host is $(uname -m)."
[ -x "$LNS" ] || skip "no lns binary at ${LNS}; build one with \`make build-lns\`."

"$LNS" service status >/dev/null 2>&1 || skip "the lns service is not running; start it with \`lns service start\`."

RUN_NAME="netstack-smoke-$$"
PROJECT="$(mktemp -d)"
trap 'rm -rf "$PROJECT"' EXIT

cat >"$PROJECT/lns.yaml" <<YAML
apiVersion: lns.run/v1
kind: sandbox
name: ${RUN_NAME}
spec:
  image: ${IMAGE}
  egress:
    http:
      - match: "*"
        verdict: allow
YAML

printf '[test]  a guest reaches a host through the guest proxy\n'
if ! OUTPUT=$("$LNS" run --rm --yes --quiet --name "$RUN_NAME" \
  --entrypoint /bin/sh -f "$PROJECT/lns.yaml" -- \
  -c 'curl -sS -o /dev/null -w "%{http_code} %{ssl_verify_result}" https://example.com; echo; cat /etc/resolv.conf; ip -4 addr show eth0'); then
  fail "the run did not complete; the guest has no working network. Output: ${OUTPUT}"
fi

case "$OUTPUT" in
  *"200 0"*) pass "the guest reached example.com over TLS through the guest proxy" ;;
  *) fail "expected \`200 0\` from example.com, got: ${OUTPUT}" ;;
esac

case "$OUTPUT" in
  *"nameserver 192.168.127.1"*) pass "the guest resolver is the netstack gateway" ;;
  *) fail "expected the netstack gateway as the guest resolver, got: ${OUTPUT}" ;;
esac

case "$OUTPUT" in
  *"192.168.127.2/24"*) pass "the guest leased 192.168.127.2/24" ;;
  *) fail "expected the guest to hold 192.168.127.2/24, got: ${OUTPUT}" ;;
esac

printf '[test]  a dual-stack host is reached over its A record\n'
if ! DUAL=$("$LNS" run --rm --yes --quiet --name "${RUN_NAME}-dual" \
  --entrypoint /bin/sh -f "$PROJECT/lns.yaml" -- \
  -c 'curl -sS -o /dev/null -w "%{http_code} %{remote_ip}" https://example.com'); then
  fail "the dual-stack fetch did not complete. Output: ${DUAL}"
fi

case "$DUAL" in
  *"200 "*) pass "example.com answered over IPv4: ${DUAL}" ;;
  *) fail "expected 200 from the dual-stack host, got: ${DUAL}" ;;
esac

printf '[test]  a host listener sees no accept from the guest\n'
WITNESS_PORT="${LNS_SMOKE_WITNESS_PORT:-47123}"
WITNESS_LOG="${PROJECT}/witness.log"
( while true; do
    printf 'HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nREACHED!' | nc -l 127.0.0.1 "$WITNESS_PORT" >>"$WITNESS_LOG" 2>&1 || break
    printf 'accepted\n' >>"$WITNESS_LOG"
  done ) &
WITNESS_PID=$!
trap 'kill "$WITNESS_PID" 2>/dev/null; rm -rf "$PROJECT"' EXIT
sleep 1

printf '[test]  nothing on the guest subnet answers but the resolver\n'
PROBES=$(cat <<EOF
probe() {
  out=\$(curl -sS --max-time 5 -o - -w '\nSTATUS=%{http_code}' "\$2" 2>/dev/null || true)
  body=\$(printf '%s' "\${out%STATUS=*}" | tr -d '\r\n')
  printf '%s-STATUS %s\n' "\$1" "\${out##*STATUS=}"
  printf '%s-BODY [%s]\n' "\$1" "\$body"
  printf '%s' "\$body" | grep -q REACHED && printf '%s-ANSWERED\n' "\$1"
  return 0
}
probe GATEWAY http://192.168.127.1/
probe HOST-NAT http://192.168.127.254:${WITNESS_PORT}/
probe LOOPBACK http://127.0.0.1:${WITNESS_PORT}/
echo done
EOF
)

if ! REFUSED=$("$LNS" run --rm --yes --quiet --name "${RUN_NAME}-refused" \
  --entrypoint /bin/sh -f "$PROJECT/lns.yaml" -- -c "$PROBES"); then
  fail "the refusal run did not complete. Output: ${REFUSED}"
fi

status_of() {
  printf '%s\n' "$REFUSED" | sed -n "s/^$1-STATUS //p"
}

GATEWAY_BODY=$(printf '%s\n' "$REFUSED" | sed -n 's/^GATEWAY-BODY \[\(.*\)\]$/\1/p')
if [ -n "$GATEWAY_BODY" ]; then
  fail "the gateway served a body on TCP; there must be no control API on it: [${GATEWAY_BODY}]"
fi
pass "http://192.168.127.1/ served nothing on TCP; the guest proxy answered $(status_of GATEWAY)"

case "$REFUSED" in
  *HOST-NAT-ANSWERED*) fail "192.168.127.254 answered; there must be no NAT entry to the host's loopback." ;;
  *) pass "http://192.168.127.254:${WITNESS_PORT}/ is refused; the guest proxy answered $(status_of HOST-NAT)" ;;
esac

case "$REFUSED" in
  *LOOPBACK-ANSWERED*) fail "127.0.0.1 answered; the guest must not reach the host's loopback." ;;
  *) pass "http://127.0.0.1:${WITNESS_PORT}/ is refused; the guest proxy answered $(status_of LOOPBACK)" ;;
esac

if [ -s "$WITNESS_LOG" ] && grep -q accepted "$WITNESS_LOG"; then
  fail "the host listener accepted a connection from the guest: $(cat "$WITNESS_LOG")"
fi
pass "the host listener saw no accept at all"

printf '[test]  the audit chain names the backend that served the link\n'
AUDIT=$("$LNS" audit "$RUN_NAME") || fail "\`lns audit ${RUN_NAME}\` failed."
case "$AUDIT" in
  *"guest network served by netstack"*) pass "the chain names netstack for this run" ;;
  *) fail "expected a netstack network line in \`lns audit ${RUN_NAME}\`, got: ${AUDIT}" ;;
esac

printf '[ok]    the guest network came from the in-process netstack\n'
