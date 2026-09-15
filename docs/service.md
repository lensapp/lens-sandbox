# The background service

`lns` is a thin command-line client. The real work happens in `lns-service`, a
small background process. On macOS the native LNS app supplies its menu-bar
interface; on Linux the service supplies its own tray interface. The service owns:

- the microVM lifecycle (booting, attaching, tearing down runs),
- the OCI image and layer caches,
- pending approvals and their decisions,
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

On macOS, the LNS app opens **Sandboxes**, **Connectors**, **Registries**,
**Audit**, **Approvals**, and **Live Requests** from its menu bar. Closing a
window or choosing **Quit Interface** keeps the service and workloads running.
**Stop Service and Quit LNS…** asks before stopping them.

On Linux, the tray menu opens **Audit** and **Approvals**, and **Quit LNS**
stops the service. Approvals shows waiting questions and their archive; entries
can be answered there or with [`lns approval`](cli-reference.md#lns-approval).

If `lns run` reports that it can't reach the service, start it with
`lns service start`.

### Socket and binary locations

On macOS the service listens on a local Unix socket at
`<data-dir>/run.lns/service.sock`.

`lns service start` looks for the `lns-service` binary next to the `lns` binary.
On macOS both live in `LNS.app/Contents/Helpers`. A service started from the bundle
opens that app; an app launch starts those same helpers. A different running
service version is rejected without stopping its workloads.
These environment variables override the defaults (mostly useful for development):

- `LNS_SOCKET_PATH` — use a specific socket path.
- `LNS_SERVICE_BIN` — use a specific `lns-service` binary.
- `LNS_HEADLESS=1` — suppress automatic interface launch (macOS) or the tray
  and approval window (Linux). An explicitly opened native app can still connect.
  Without a connected interface, use the terminal:
  `lns approval ls` shows what a run has been asked, and `lns approval answer`
  answers it. A held request still fails closed while it waits, so the answer
  decides the next attempt rather than the one that raised it. Pre-authorized
  rules in a mixin the run names avoid the question altogether.

## Updating

Update to the latest release:

```bash
lns update
```

On macOS this updates the whole signed app, including its CLI and service.
The download is checksum-verified, signature-verified, and checked by Gatekeeper
before replacing the installed app. Updating closes the interface and stops the
service, interrupting running sandboxes. Existing login startup is preserved;
if replacement or startup fails, the installer restores the previous app and
CLI links. On Linux the updater replaces the two binaries.

For an installation from before the native app release, run the current
[installer](getting-started.md#install) once to migrate. Older CLI updaters do
not understand the native app archive.

Force a reinstall even when the running version already matches — for example if a
binary is corrupt or its code signature was invalidated:

```bash
lns update --force
```

## See also

- [Getting started](getting-started.md) — installing and starting the service.
- [CLI reference](cli-reference.md) — `lns service` and `lns update`.
