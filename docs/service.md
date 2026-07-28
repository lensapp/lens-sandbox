# The background service

`lns` is a thin command-line client. The real work happens in `lns-service`, a
small background process that runs in your menu bar / system tray. It owns:

- the microVM lifecycle (booting, attaching, tearing down runs),
- the OCI image and layer caches,
- the approval window and credential prompts,
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

## The dashboard

The **Dashboard** item in the tray menu opens a window with two views over
everything the service knows:

- **Credentials** — every [connector](connectors.md) this machine has a value
  decision for, what it is bound to, which sandboxes can spend it, and what
  destinations it reaches. Requests waiting for you appear at the top and can be
  answered here instead of from the approval window.
- **Audit** — the same [audit](audit.md) timeline `lns audit` prints, filtered by
  event kind or by sandbox.

Selecting a sandbox in the sidebar scopes both views to it. `⌘K` searches
credentials and activity together.

Credential values are never shown. The window discloses what is bound (signed
in, stored on this machine, host value) along with the account, scopes, expiry,
and the fake placeholder the workload holds — never the real secret, which stays
in `~/.lns-credentials.json` and is injected at the boundary.

Three actions write through to the same files the CLI edits:

| Action | Equivalent to |
| --- | --- |
| **Replace** a stored value | `lns connector connect <id>` with a new value |
| **Remove credential** | forgetting the machine-wide value decision |
| **Disconnect** | `lns connector disconnect <id>` for that sandbox's project |

**Disconnect** removes the connector from the project's `lns-policy.yaml` and
forgets that project's per-workload grants, so the next use asks again. A
connector a sandbox definition *requires* can't be disconnected this way — the
launch depends on it.

### Socket and binary locations

On macOS the service listens on a local Unix socket at
`<data-dir>/com.lensapp.sandbox/service.sock`.

`lns service start` looks for the `lns-service` binary next to the `lns` binary.
Two environment variables override the defaults (mostly useful for development):

- `LNS_SOCKET_PATH` — use a specific socket path.
- `LNS_SERVICE_BIN` — use a specific `lns-service` binary.
- `LNS_HEADLESS=1` — run without the tray or approval window even when a
  display is present. Interactive prompts can't be shown headless: approvals
  need pre-authorized rules in `lns-policy.yaml`, and `lns connector
  connect` for a credential connector fails immediately instead of waiting
  on a card that can never appear.

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
