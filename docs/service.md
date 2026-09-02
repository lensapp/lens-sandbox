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

You can also quit it from the **Quit** item in its tray menu.

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
  display is present. Interactive prompts can't be shown headless, so
  approvals need pre-authorized rules in `lns-local-mixin.yaml`.

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
