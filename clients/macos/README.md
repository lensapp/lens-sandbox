# Native macOS client

This client provides sandbox controls, connector management, an audit dashboard,
approval history, and live network approvals using SwiftUI and AppKit. It communicates with
`lns-service` through the same local IPC boundary available to other clients.
It does not read run directories, write policy, or embed the Rust service.

The interface uses a dark appearance with charcoal surfaces, blue accents,
compact system typography, and muted status badges. Sandboxes use a list;
connectors use bordered cards. The same styling carries through registry sign-in,
creation dialogs, audit details, and live approvals. Native controls and keyboard
shortcuts remain available. Shared colors and presentation components live in
`Sources/LNSMac/LNSTheme.swift`.
Page headers, search fields, empty states, and persistent form labels share
presentation components. The sandbox list uses matching header and row columns;
connector cards highlight Connect until access is available, then Grant Access.

Audit and Approvals each include a sandbox filter in their own view; the sidebar
contains navigation only. The native dashboard includes event-kind filters, global
audit search, event details and copy controls, integrity warnings, and approval
history grouped into waiting requests and an archive. History rows offer the
service's persistent answers, connector grants, and removal from the list.

The release installer has not been migrated. This app remains a development
evaluation, not the default shipping macOS interface.

## Desktop controls

The dashboard opens on **Sandboxes**, a searchable list of running and stopped
sandboxes. Start or stop a sandbox, open its activity or approvals, or choose
**Grant Connector Access…** to select a connector for it. Removing a stopped
sandbox asks first and deletes its writable layer and decisions. Removed
sandboxes remain available in audit history but are excluded from the controls.

**New Sandbox…** (⌘N) starts a fresh sandbox from a local `lns.yaml`, a folder
containing one, or a published reference. Give it an optional name and choose
**Start Sandbox**. The app shows launch details and returns to the list once the
workload is ready. Startup failures keep their details visible for review.
The setup checkbox accepts the definition's declared installers, scripts,
mounts, and host access, using the CLI's `--yes` behavior. It starts unchecked;
when consent is required, review the reported details before enabling it and
retrying. Creating a sandbox requires the bundle containing CLI/service helpers.
Use **Sign In to a Registry…** in this form if its definition or base image
requires authentication; then retry the launch after signing in.

**Connectors** shows installed connectors as cards with saved connections and
their authority. An unconnected card leads with **Connect…**; once a connection
is available, **Grant Access…** becomes primary and **New connection…** remains
available. Access that needs no account can be granted directly. Install a registry
reference or choose a local connector document. **Connect…** saves a named
connection using the method's credential fields; it does not grant a sandbox
access. Connection names must be new, so adding an account cannot replace an
existing connection by accident.

**Grant Access…** asks for a sandbox and a saved connection; the connection
determines its method. Options that need no account appear alongside connections.
The form discloses destinations, files, variables, and connection authority before
you grant. **Add Connection…** returns to the grant form after connecting. A grant
replaces that sandbox's previous grant for the connector. The app rechecks the
connector inventory before submitting; if it changed, reopen the form to review
the current offer. Where the service does not provide sandbox-specific deny-rule
overrides, the form says they could not be checked.

The card menu offers **Forget Sandbox Decision…**, which makes the sandbox ask
again on its next start. Disconnecting removes a saved connection; uninstalling
removes the connector and its connections. Both ask first and leave existing
sandbox grants in place. Refresh with ⌘R after changing connectors in another
client.

Use **Navigate → Audit** (⌘1), **Approvals** (⌘2), or **Live Requests** (⌘3).
**Sandboxes** (⌘4) and **Connectors** (⌘5) are also available from Navigate and the
menu-bar interface.
**Registries** (⌘6) lists saved registry accounts. **Sign In…** offers browser
sign-in through the bundled CLI, displaying the confirmation code while you
approve it, or username and password/token entry for registries such as GHCR.
The service verifies credentials before saving them in the same store the CLI
uses. **Sign Out…** removes the saved account after confirmation. Browser
sign-in can be canceled; failed sign-ins are shown without retrying automatically.
The Dock and menu bar use the original LNS logo. The menu-bar image remains a
template so macOS adapts its color to the current appearance.
The Dock image is assigned at startup even when launching the app executable
directly from a terminal. If an icon cannot load, the menu bar shows **LNS** and
the app reports the missing or invalid asset.
⌘F focuses search in Sandboxes, Connectors, and Audit; ⌘R refreshes the dashboard. Escape clears a focused
search or closes focused event details. Standard macOS window controls and ⌘W
close a window without stopping the service; the menu-bar interface stays open.
⌘Q quits only the interface. Stopping the service is a separate, confirmed
action because it interrupts running sandboxes.

The interface clears actionable data when disconnected. Overlapping refreshes
are coalesced into a fresh post-action read, and canceled reads cannot restore
an old snapshot. A finite read that stops delivering complete frames times out;
established subscriptions remain idle without polling.

## Self-contained development bundle

On macOS, build a bundle containing the native interface and release-built
CLI/service helpers:

```sh
make -C clients/macos package
make -C clients/macos smoke
```

This produces `clients/macos/dist/LNS.app` and `LNS-macos.zip`. Copy the app to
your user-owned Applications directory, or run it from `dist`. On opening, the
app runs the bundled `lns service start` against the interface's socket, with the
Rust UI disabled. That command reuses an already-running service. If startup
fails, the app shows the error and offers **Start Service** to retry. It does not register
a login agent, replace a separately installed CLI, or stop an existing service.
An already-running service must be from the matching build of this branch.

The helpers live in `LNS.app/Contents/Helpers/`. Use that `lns` executable to run
workloads against the same socket. The bundled helper refuses `lns update` and
`lns uninstall` before changing anything: those commands manage loose binaries,
not signed app bundles. `lns update --dry-run` remains read-only and available.
Update this evaluation by replacing the entire app after stopping its service;
remove it by moving the entire app to Trash. Neither operation removes run data.

Packaging signs helpers before the enclosing app and verifies the signatures
before replacing an existing bundle. Each previous build is retained in a
printed `.lns-previous.*` directory under `dist` for recovery; remove those
development backups when no longer needed. `VERSION` defaults to the CLI crate
version and must agree with the packaged helper. `SIGN_IDENTITY` selects ad-hoc
signing (the default) or a configured Developer ID identity with hardened runtime
and timestamping. Notarization and automatic app updates are not configured.

Branch pushes run a macOS build and bundle smoke check, then retain a zipped app
as a GitHub Actions artifact for seven days. These are development artifacts,
not notarized public releases. The smoke check uses an isolated temporary home
and socket; it does not start a guest or touch installed service data.

## Build and run

On a Mac with Xcode, from the repository root:

```sh
make dev
make -C clients/macos verify
```

Start the matching service build with its existing `LNS_HEADLESS=1` option to
evaluate the native app without also displaying egui cards. Use a dedicated
socket directory and data home to avoid sharing run state with an installed
service. The socket directory is made private; do not put the socket directly
in a shared directory such as `/tmp`.

```sh
LNS_HOME="$HOME/.lns-native-evaluation/data" \
LNS_SOCKET_PATH="$HOME/.lns-native-evaluation/service.sock" \
LNS_HEADLESS=1 target/debug/lns-service
```

In another terminal:

```sh
LNS_SOCKET_PATH="$HOME/.lns-native-evaluation/service.sock" \
clients/macos/dist/LNS.app/Contents/MacOS/LNS
```

Point the matching CLI at that socket and data home as well:

```sh
LNS_HOME="$HOME/.lns-native-evaluation/data" \
LNS_SOCKET_PATH="$HOME/.lns-native-evaluation/service.sock" target/debug/lns ps
```

The isolated data home starts empty. Running a real workload from a debug
service also requires prebuilt guest binaries. `make build` embeds them and
signs the macOS service for virtualization; use `bin/lns-service` and `bin/lns`
in the commands above to test real workloads with that release build.

The menu's **Audit** and **Approvals** items open the dashboard. **Live Requests**
opens the same desktop approval overlay used for new held requests. It is
borderless, uses the app's dark styling, and grows to fit its cards up to a
scrollable height limit. New requests appear without taking keyboard focus;
opening Live Requests explicitly focuses the overlay. It works with the dashboard
closed and across Spaces, including fullscreen apps.

Network cards show **Always Deny** and **Always Allow**. Their action menu contains
**Allow Once**, **Deny Once**, and **Dismiss Request**. **View details** opens
the request in the main app's Approvals view with its full action and connector
access details. Connector notifications start with the first available saved
connection ready for **Grant Access**. The picker also offers other saved
connections, access that needs no account, and **New connection…**. Without
available access, the new connection form opens directly: enter a name and
credentials inline and choose **Connect & Grant**. Changing the selection clears unsubmitted
credentials. **Skip Connector** stays in the card's action menu.
Closing the overlay or pressing Escape hides it without answering any requests.
Reopen it from Live Requests (⌘3).

Audit details have one **Copy event** action for the raw event. Individual values
remain selectable and offer Copy in their context menus.

Without an override, the app uses
`~/Library/Application Support/run.lns/service.sock`.

The app bundle is signed ad hoc for local evaluation. `SIGN_IDENTITY` selects
another signing identity. Release notarization, installer integration, and
coordinated app/service updates are still required before shipping it.

## Client contract

- `WatchDashboard` signals an initial refresh and later service writes. The app
  coalesces bursts and does not poll while idle.
- `ReadDashboard` returns a finite sequence: `DashboardBegin`, individual
  sandbox/approval/event/warning frames, then `DashboardEnd`. The client keeps
  every frame and publishes only a completed read. A large timeline is not one
  oversized frame; an interrupted read is an error, not an empty dashboard.
  `DashboardEnd` completes the request without waiting for a socket EOF.
- `InspectApprovalOffer` reads the offer a history row still holds.
  `GrantApproval` includes the disclosed digest and is refused if it changed.
  An acknowledgment reports handling, not proof a grant persisted; refreshed
  history and live notices carry the outcome.
- `WatchApprovals` streams complete `LiveApprovals` snapshots, including an
  initial snapshot. The JSON for each snapshot travels in bounded
  `LiveApprovalsChunk` frames with UTF-8 byte offsets and a final `complete`
  marker. Clients reassemble before publishing or coalescing snapshots; a
  missing, reordered, or interrupted chunk cannot publish partial state.
  Slow clients can skip intermediate completed snapshots without
  missing the current state. Reconnecting starts with current state again.
  Notice dismissal uses bounded batches of exactly the notices observed.
  A single notice too large for a dismissal request is reported without sending
  any batches; it does not prevent receiving or answering approvals.
- Each approval has a stable `id` for view identity and an opaque `token` for
  responding to its current presentation. Hold expiry changes the token but
  preserves the form's identity.
- `RespondToApproval` names that token and an action. `LiveApprovalSubmitted`
  means queued for the run, not that a policy write or grant succeeded. Further
  snapshots carry resulting state and notices. `LiveApprovalStale` means the
  presentation changed or another client already submitted an answer.
- Live actions are distinct from `ListApprovals` / `AnswerApproval`, which
  operate on the history of questions a run keeps.
- Disconnecting clears the app's visible snapshot and disables answering. It
  does not dismiss service approvals or retry commands. Credential values are
  cleared from the input controls after submission and are never logged.
- Closing the overlay hides it. **Dismiss Request** explicitly fails the held
  request without recording a decision. **Quit Interface** leaves the service
  running; **Stop Service and Quit LNS** sends `Shutdown`.
  New failure notices also raise the overlay, even after the last request is gone;
  unchanged notices do not repeatedly bring a manually hidden overlay forward.

## Verification

On macOS, `make -C clients/macos test` also hosts native views to check approval
resizing, its scrolling limit, Escape/reopen behavior, and audit details at narrow
widths without contacting a service. These checks complement visual inspection of the app.

`make -C clients/macos test` tests framing, dashboard replacement and filtering,
history requests, management commands and outcomes, sandbox creation, registry
sign-in and cancellation, automatic service startup, explicit grant selection,
stale connector disclosures, duplicate actions, reconnect state, and shared
Rust/Swift wire fixtures.
`make -C clients/macos launch-smoke` checks the bundled CLI invocation and drains
both output streams through real subprocesses, including a failed launch and
a browser confirmation code that must arrive before the helper finishes.
On macOS, `make -C clients/macos icon-smoke` decodes and renders the packaged
icons from a relocated app bundle and checks missing-resource reporting.
Those Foundation-only tests also run on Linux with Swift installed. CI runs
`verify` on macOS when the native client or its service contract changes.

Before replacing the existing macOS interface, verify a live approval through
the actual service and guest, simultaneous clients, reconnect after service
restart, connector expiry while typing, failed policy writes, VoiceOver,
keyboard navigation, Spaces, fullscreen, multiple displays, idle resource
usage, and installation/update behavior. Native platform checks cannot be
substituted by the Linux test result.
