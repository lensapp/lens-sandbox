# The background service

`lns` is a thin command-line client. The real work happens in `lns-service`, a
small background process that runs in your menu bar / system tray. It owns:

- the microVM lifecycle (booting, attaching, tearing down runs),
- the OCI image and layer caches,
- the approval window,
- the per-run [audit](audit.md) writer.

`lns run`, `lns ps`, `lns exec`, and `lns stop` all talk to it over a local Unix
socket. It's a user-launched process you start yourself — not a system daemon — and
it's meant to stay running so the sandbox is always ready.

## Managing the service

```bash
lns service start    # launch it and wait until it's ready
lns service status   # report whether it's running (PID, uptime, version)
lns service stop     # ask it to shut down
```

Its tray menu opens two views and quits the service. The **Audit** view shows the
timeline of what the runs did. The **Approvals** view shows what they were
asked, one row per question, gathered under the sandbox that was asked. A mark
names each question. Point to the mark to read what it asks. The list holds
what the runs are waiting on. **Archive** holds the rest, behind one click, and
is open already when nothing waits. Open a row to see the action its card
showed, and everything that answers it. An open row answers the entry, and
files it in the Archive, the way
[`lns approval`](cli-reference.md#lns-approval) answers one. Choose a sandbox or an
answer to narrow the list. An open connector row offers the grant its card
offered, so a connector card you closed is answered there too. **Quit** stops
the service.

If `lns run` reports that it can't reach the service, start it with
`lns service start`.

### Socket and binary locations

On macOS the service listens on a local Unix socket at
`<data-dir>/run.lns/service.sock`.

`lns service start` looks for the `lns-service` binary next to the `lns` binary.
Two environment variables override the defaults (mostly useful for development):

- `LNS_SOCKET_PATH` — use a specific socket path.
- `LNS_SERVICE_BIN` — use a specific `lns-service` binary.
- `LNS_HEADLESS=1` — run without the tray or approval window even when a
  display is present. No card can be shown, so the terminal is the only surface:
  `lns approval ls` shows what a run has been asked, and `lns approval answer`
  answers it. A held request still fails closed while it waits, so the answer
  decides the next attempt rather than the one that raised it. Pre-authorized
  rules in a mixin the run names avoid the question altogether.

## The guest network

Each run gets its own network, served inside the service by a small
userspace TCP/IP stack that starts with the guest and stops with it. There is
no bridge on the host, no host DHCP server, no helper binary, and no state
shared between runs.

On macOS the service creates a datagram socket pair, hands one end to the
virtual machine as its network device, and keeps the other. It then speaks
ethernet on that socket itself: it answers ARP for the gateway, leases the
guest its address over DHCP, relays DNS on the gateway, and turns the guest's
TCP and UDP flows into ordinary host sockets.

The link is **IPv4 only**. IPv6, VLAN-tagged frames and IPv4 fragments are
dropped and counted; the gateway does no reassembly. It carries TCP and UDP,
and answers ICMP echo at the gateway alone: a guest cannot ping an address on
the internet, which the `vmnet` bridge did carry.

| | |
|---|---|
| Guest subnet | `192.168.127.0/24` |
| Gateway and DNS resolver | `192.168.127.1` |
| Guest address | `192.168.127.2` |
| Gateway MAC | `0e:6c:6e:73:00:01` (locally administered) |
| MTU | 1500, both ways |

Egress policy is unaffected. Every request still goes through the in-guest
proxy, so the approval cards and the [audit](audit.md) chain read the same as
before. The chain also records which backend a run used:

```
$ lns audit
2026-09-11T09:14:02Z  calm-finch  network  guest network served by netstack (192.168.127.0/24)
```

The launch prints the same thing:

```
$ lns run -- curl -sS https://example.com
  Network  netstack (192.168.127.0/24)
  Booting  microVM
```

### What the guest can reach

The stack decides every destination before it opens a host socket — before
the stack itself sees the packet. A refused TCP segment is answered with a
reset and a refused datagram with an ICMP port-unreachable, so the guest
fails at once instead of waiting out a timeout.

| Destination | Result |
|---|---|
| `192.168.127.1:53` (UDP and TCP) | The gateway relays the query. Over TCP the message carries the two-byte length prefix of RFC 1035. |
| `192.168.127.1`, ICMP echo | The gateway answers, so a guest can tell a dead link from a refused destination. |
| Anything else in `192.168.127.0/24` | Refused. There is no control API on the gateway and no address that forwards to the host. |
| `127.0.0.0/8` | Refused. The guest cannot reach anything bound to the host's loopback. |
| `0.0.0.0/8`, `169.254.0.0/16`, `224.0.0.0/4`, `255.255.255.255` | Refused. |
| ICMP to anything but the gateway | Dropped; only TCP and UDP are carried. `ping 1.1.1.1` from the guest gets nothing, where the `vmnet` bridge forwarded it. |
| Every other address, including the host's own LAN addresses | TCP and UDP are carried, exactly as they were on the `vmnet` bridge. |

Published ports (`-p`, `spec.ports`) are unaffected: they travel over the
run's vsock channel, not over this link.

### DNS

The gateway is a plain relay. It does not resolve anything itself and caches
nothing: your host's answer, its TTLs and its negative answers are what the
guest gets.

The resolver list is your host's own — the nameservers of
`/etc/resolv.conf`, plus the per-domain resolvers `scutil --dns` reports.
That second source matters: a split-DNS VPN's internal resolvers appear only
there, never in `resolv.conf`. The longest matching domain suffix decides
which servers answer a name; every resolver that ties for it is asked, lowest
`order` first, and each is asked on the port its own configuration names. A
name no suffix covers goes to the default ones. The list is read again every
30 seconds, and again after any query nobody answered, so a VPN that comes up
mid-run is picked up. Every read runs off the runtime's workers, the first
one included, so neither a query nor the start of a run waits for it. A
query put before the first read has landed gets SERVFAIL, and the next one
reads again. A read has 2 seconds; one that takes longer is dropped, with
the `scutil` it was waiting on, and the list in hand is kept. A read that
names no server at all is a host mid-change, so the list in hand is kept
then too, and the service says so once.

Each query goes to the first server over UDP, and to the next after 700 ms
while the first is still pending; the first usable answer wins. SERVFAIL,
REFUSED and NOTIMP move to the next server the same way an error does;
NOERROR and NXDOMAIN are answers and come back as they are. The whole search
has 1800 ms, under the 2 seconds the guest's own stub waits. A server that
answers nothing is asked last for the next 30 seconds. A truncated answer is
asked again over TCP to the same server. When no server answers, the guest
gets SERVFAIL — never silence.

The gateway answers on UDP and on TCP. Over UDP it fits the answer to what
the guest said it can take: the payload size of the query's OPT record, or
512 bytes when the query carries none, and never more than 1472 bytes — one
frame of this link. An answer that does not fit comes back with its question,
its OPT record and the TC bit set, per RFC 2181 §9, so the guest asks again
over TCP. Over TCP the gateway serves the same relay, with the two-byte
length prefix of RFC 1035 §4.2.2: one connection carries one or more queries,
and an idle connection is closed after 10 seconds. These connections have an
allowance of their own, apart from the guest's TCP flows: 64 at a time, 64
queries each, and an answer that does not reach the guest in 5 seconds closes
the connection. A guest cannot fill its own flow table with idle connections
to the resolver.

### Bounds

One guest holds no more of the host than this:

| | |
|---|---|
| Concurrent TCP flows | 1024 |
| Concurrent UDP flows | 512 |
| DNS queries in flight | 256 |
| Concurrent DNS connections over TCP | 64 |
| Queries per DNS connection | 64 |
| TCP connect timeout | 10 s |
| TCP buffer per direction per flow | 256 KiB |
| UDP flow idle timeout | 60 s |
| DNS connection idle timeout | 10 s |
| DNS answer write timeout | 5 s |
| Largest DNS answer over UDP | 1472 bytes |
| Frames queued between the device and the stack | 512 |

Anything over a limit is dropped and counted, and the count is written to the
developer trace stream (`lns run --debug`). A guest that outruns the stack
loses frames, as it would on a busy wire.

### Where the network variables are set

`LNS_GUEST_SUBNET` and `LNS_NETDEV` are read by the service, from its own
environment, when it serves a link. They are not part of what `lns run` sends
over the socket, so setting one for a run changes nothing. `lns run` says so
when it sees one:

```
$ LNS_NETDEV=vmnet lns run -- curl -sS https://example.com
warning: LNS_NETDEV takes effect on lns-service, not on this run. Set it where the service starts: `lns service stop`, then start the service again with the variable in its environment.
```

Set one on the service instead. `lns service start` passes its own environment
to the service it launches:

```bash
lns service stop
LNS_NETDEV=vmnet lns service start
```

Every run served by that service uses the setting, until the service stops.
A service the login agent starts at your next login does not carry it, because
the agent holds no environment of yours: set it again the same way.

To run the service in the foreground with the setting instead, start the
binary yourself and leave the terminal open:

```bash
LNS_NETDEV=vmnet "$(dirname "$(command -v lns)")/lns-service"
```

### Moving the guest subnet

`LNS_GUEST_SUBNET=<a /24>` replaces `192.168.127.0/24`, for hosts that
already use it. The gateway is always `.1` and the guest `.2`, and the
boundary policy follows:

```bash
lns service stop
LNS_GUEST_SUBNET=10.99.7.0/24 lns service start
```

The value must be a `/24` whose last octet is `0`. If the host holds an
address inside the guest subnet, the service says so at startup: that address
is one no guest can reach, because the boundary refuses the guest's own
subnet whole.

### Going back to the old bridge

`LNS_NETDEV=vmnet` restores the previous behaviour — the Apple `vmnet` NAT
bridge, with the host answering DHCP:

```bash
lns service stop
LNS_NETDEV=vmnet lns service start
```

This is an escape hatch for one release. Report anything that needs it.
`LNS_NETDEV=netstack` names the default explicitly. A run whose link cannot be
served fails before the workload starts and exits `125`, like every other
pre-start refusal.

## Updating

Update both binaries to the latest release:

```bash
lns update
```

Force a reinstall even when the running version already matches — for example if a
binary is corrupt or its code signature was invalidated:

```bash
lns update --force
```

## See also

- [Getting started](getting-started.md) — installing and starting the service.
- [CLI reference](cli-reference.md) — `lns service` and `lns update`.
