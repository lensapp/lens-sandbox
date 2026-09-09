# Native macOS client

This client provides an audit dashboard, approval history, live network
approvals, and connector grants using SwiftUI and AppKit. It communicates with
`lns-service` through the same local IPC boundary available to other clients.
It does not read run directories, write policy, or embed the Rust service.

The native dashboard includes sandbox selection, event-kind filters, global
audit search, event details and copy controls, integrity warnings, and approval
history grouped into waiting requests and an archive. History rows offer the
service's persistent answers, connector grants, and removal from the list.

The release installer has not been migrated. This app remains a development
evaluation, not the default shipping macOS interface.

## Desktop controls

Use **Navigate → Audit** (⌘1), **Approvals** (⌘2), or **Live Requests** (⌘3).
⌘F focuses audit search and ⌘R refreshes the dashboard. Escape clears a focused
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
your user-owned Applications directory, or run it from `dist`. Choose **Start
Service** when disconnected. It explicitly runs the bundled `lns service start`
against the interface's socket, with the Rust UI disabled. It does not register
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
opens the live approval list; new held requests also raise the floating panel.

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
- `InspectApprovalOffer` reads the offer a history row still holds.
  `GrantApproval` includes the disclosed digest and is refused if it changed.
  An acknowledgment reports handling, not proof a grant persisted; refreshed
  history and live notices carry the outcome.
- `WatchApprovals` streams complete `LiveApprovals` snapshots, including an
  initial snapshot. Slow clients can skip intermediate snapshots without
  missing the current state. Reconnecting starts with current state again.
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
- Closing the panel hides it. **Dismiss Request** explicitly fails the held
  request without recording a decision. **Quit Interface** leaves the service
  running; **Stop Service and Quit LNS** sends `Shutdown`.

## Verification

`make -C clients/macos test` tests framing, dashboard replacement and filtering,
history requests, reconnect state, and shared Rust/Swift wire fixtures.
Those Foundation-only tests also run on Linux with Swift installed. CI runs
`verify` on macOS when the native client or its service contract changes.

Before replacing the existing macOS interface, verify a live approval through
the actual service and guest, simultaneous clients, reconnect after service
restart, connector expiry while typing, failed policy writes, VoiceOver,
keyboard navigation, Spaces, fullscreen, multiple displays, idle resource
usage, and installation/update behavior. Native platform checks cannot be
substituted by the Linux test result.
