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
virtual machine. On Linux it is [`passt`](https://passt.top), started per run in
`--vhost-user` mode; `cloud-hypervisor` connects to the socket `passt` binds and
gives the guest a `virtio-net` device on the other end of it. Either way the
backend answers DHCP, DNS and NAT itself:

| | |
|---|---|
| Guest subnet | `192.168.127.0/24` |
| Gateway and DNS resolver | `192.168.127.1` |
| Guest address | `192.168.127.2` (Linux; macOS leases from the same subnet) |
| MTU | 1500 |

`passt` would otherwise hand the guest the host's own address and default
gateway, so lns asks for that private address instead — the same subnet the
macOS backend serves, so a guest reads the same on both platforms. The gateway
address is not mapped back to the host's loopback (`--no-map-gw`), so nothing in
the guest can reach a service listening on the host by aiming at its gateway.
DNS is forwarded by `passt` itself from the address it advertises, so the
resolver the guest is handed is always one that answers.

Egress policy is unaffected. Every request still goes through the in-guest
proxy, so the approval cards and the [audit](audit.md) chain read the same as
before. The chain also records which backend a run used:

```
$ lns audit
2026-09-11T09:14:02Z  calm-finch  network  guest network served by passt (192.168.127.0/24)
```

The launch prints the same thing:

```
$ lns run -- curl -sS https://example.com
  Network  passt (192.168.127.0/24)
  Booting  microVM
```

### Where gvproxy comes from (macOS)

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

### Where passt comes from (Linux)

`passt` publishes no versioned static binary with a checksum to pin, so lns does
not download one. It uses the `passt` the host has, looking in three places and
stopping at the first hit:

1. `LNS_PASST_BIN` — a path you name. If that path is not a file, the run is
   refused; it does not fall back.
2. `passt` on `PATH`.
3. `/usr/bin`, `/usr/local/bin`, `/usr/sbin`, `/usr/local/sbin` — where the
   distro packages put it.

A host with no `passt` refuses the run before the workload starts and exits
`125`, naming the package:

```bash
sudo apt install passt    # Debian, Ubuntu
sudo dnf install passt    # Fedora, RHEL
```

`lns-install.sh` probes for it next to `cloud-hypervisor` and `virtiofsd` and
says so during install.

### Running the service in a container

`passt` isolates itself in a user namespace as it starts. Where `unshare(2)` is
blocked — Docker's default `seccomp` profile blocks it for a container without
`CAP_SYS_ADMIN` — `passt` reports

```
Couldn't create user namespace: Operation not permitted
```

and stops, so the run is refused rather than started without a network. There is
no option that turns the isolation off. A container that hosts `lns-service`
therefore needs `unshare(2)` allowed:

```bash
docker run --device /dev/kvm --security-opt seccomp=unconfined ...
```

Podman's default profile already allows it.

### Going back to the old behaviour

On macOS, `LNS_NETDEV=vmnet` restores the Apple `vmnet` NAT bridge, with the
host answering DHCP. On Linux, `LNS_NETDEV=none` boots the guest with no network
device at all, which is what every Linux run did before this change:

```bash
LNS_NETDEV=vmnet lns run -- curl -sS https://example.com   # macOS
LNS_NETDEV=none  lns run -- echo hello                     # Linux, no egress
```

These are escape hatches for one release. Report anything that needs them.

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
