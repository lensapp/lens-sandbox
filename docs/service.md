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

Each run gets its own network, served by a small userspace process the service
starts beside the guest and stops with it. There is no bridge on the host, no
host DHCP server, and no shared packet filter between runs.

On macOS that process is
[`gvproxy`](https://github.com/containers/gvisor-tap-vsock). The service starts
one per run, connects a datagram socket to it, and attaches that socket to the
virtual machine. `gvproxy` answers DHCP, DNS and NAT itself:

| | |
|---|---|
| Guest subnet | `192.168.127.0/24` |
| Gateway and DNS resolver | `192.168.127.1` |
| MTU | 1500 |

Egress policy is unaffected. Every request still goes through the in-guest
proxy, so the approval cards and the [audit](audit.md) chain read the same as
before. The chain also records which backend a run used:

```
$ lns audit
2026-09-11T09:14:02Z  calm-finch  network  guest network served by gvproxy (192.168.127.0/24)
```

The launch prints the same thing:

```
$ lns run -- curl -sS https://example.com
  Network  gvproxy (192.168.127.0/24)
  Booting  microVM
```

### Where gvproxy comes from

The service looks in three places, in order, and stops at the first hit:

1. `LNS_GVPROXY_BIN` — a path you name. If that path is not a file, the run is
   refused; it does not fall back.
2. `gvproxy` on `PATH`.
3. The copy lns manages, at `~/.lns/gvproxy/<version>/gvproxy`. If it is not
   there, the service downloads the pinned release on first use and checks it
   against the published SHA-256 before installing it. Bytes that do not match
   are never installed, and the run is refused.

There is no `brew` or `apt` step. `lns-install.sh` fetches the same pinned
binary during install on macOS, so the first run needs no download.

A run that cannot get a verified `gvproxy` fails before the workload starts and
exits `125`, like every other pre-start refusal.

### Going back to the old bridge

`LNS_NETDEV=vmnet` restores the previous behaviour — the Apple `vmnet` NAT
bridge, with the host answering DHCP:

```bash
LNS_NETDEV=vmnet lns run -- curl -sS https://example.com
```

This is an escape hatch for one release. Report anything that needs it.

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
