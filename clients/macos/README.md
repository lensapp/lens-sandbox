# Native macOS approval client

This is the first native client slice: live network approvals and connector
grants, presented with SwiftUI in an AppKit panel. It communicates with
`lns-service` through the same local IPC boundary available to other clients.
It does not read run directories, write policy, or embed the Rust service.

The existing dashboard and release installer have not been migrated. This app
is a development evaluation, not the default shipping macOS interface.

## Build and run

On a Mac with Xcode, from the repository root:

```sh
make dev
make -C clients/macos verify
```

Start the matching service build with its existing `LNS_HEADLESS=1` option to
evaluate the native app without also displaying egui cards. Use a separate
development socket to avoid disturbing a running installed service:

```sh
LNS_SOCKET_PATH=/tmp/lns-native-evaluation.sock LNS_HEADLESS=1 target/debug/lns-service
```

In another terminal:

```sh
LNS_SOCKET_PATH=/tmp/lns-native-evaluation.sock clients/macos/dist/LNS.app/Contents/MacOS/LNS
```

Point the matching CLI at that socket as well. Running a real workload from a
debug service also requires the guest binaries documented by the repository's
debug-build workflow; a release service embeds them.

Without an override, the app uses
`~/Library/Application Support/run.lns/service.sock`.

The app bundle is signed ad hoc for local evaluation. `SIGN_IDENTITY` selects
another signing identity. Release notarization, installer integration, and
coordinated app/service updates are still required before shipping it.

## Client contract

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

`make -C clients/macos test` tests framing and the shared Rust/Swift wire fixture.
Those Foundation-only tests also run on Linux with Swift installed. CI runs
`verify` on macOS when the native client or its service contract changes.

Before replacing the existing macOS interface, verify a live approval through
the actual service and guest, simultaneous clients, reconnect after service
restart, connector expiry while typing, failed policy writes, VoiceOver,
keyboard navigation, Spaces, fullscreen, multiple displays, idle resource
usage, and installation/update behavior. Native platform checks cannot be
substituted by the Linux test result.
