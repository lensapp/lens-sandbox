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

This is the macOS interface. The macOS service has no egui, winit, or Rust tray
rendering dependencies; Linux retains its existing desktop interface.

## Desktop controls

The dashboard opens on **Sandboxes**, a searchable list of running and stopped
sandboxes. Start or stop a sandbox, open its activity or approvals, or choose
**Grant Connector Access…** to select a connector for it. Removing a stopped
sandbox asks first and deletes its writable layer and decisions. Removed
sandboxes remain available in audit history but are excluded from the controls.

**New Sandbox…** (⌘N) starts a fresh sandbox from a local `lns.yaml`, a folder
containing one, or a published reference. **Recently Used Definitions** fills in a
source without restoring an earlier run's extra mixins. **Add Mixin…** accepts a
local file or folder, a published reference, or a recently used mixin. Move added
mixins earlier or later to control precedence. Recent sources are stored in this
interface's preferences after a successful launch; clearing a recent item removes
only its shortcut.

Choose **Review Configuration** to resolve the definition and mixins. Review their
sources, contributions, replacements, network rules, mounts, files, and scripts.
Published references are pinned for launch; changing a source clears the preview
and setup consent. Give the sandbox an optional name and choose **Start Sandbox**.
The app shows launch details and returns to the list once the
workload is ready. Startup failures keep their details visible for review.
The setup checkbox accepts the definition's declared installers, scripts,
mounts, and host access, using the CLI's `--yes` behavior. It starts unchecked;
when consent is required, review the reported details before enabling it and
retrying. Creating a sandbox requires the bundle containing CLI/service helpers.
Use **Sign In to a Registry…** in this form if its definition or base image
requires authentication; then retry the launch after signing in.

Click a sandbox's name or choose **View Configuration…** to inspect its recorded
definition and mixins, connector grants, and current network rules. **Your
Decisions** shows persistent answers and updates with service notifications. The
effective rule list identifies each source and explains precedence for identical
rule scopes. Where audit history contains a matching persistent approval, its
time is shown. One-time answers remain in Approvals history. Older runs that did
not retain attribution say so, and still show their recorded configuration and
current decisions. Declarations other than network rules in the decisions file
are shown separately because their startup effects are not applied live.

**Save Definition…** is available in the sandbox's action menu and configuration
view for running and stopped sandboxes. It writes the resolved definition and
persistent decisions to a new file, folding in mixins and excluding connector
grants. Existing files are never overwritten. A successful save adds that file
to recently used definitions.

**Connectors** shows installed connectors as cards with the author's
`spec.description`, saved connections, and their authority. Missing or blank
descriptions take no space. Search matches names, descriptions, and destinations;
grant details disclose the destinations a method opens.
An unconnected card leads with **Connect…**; once a connection
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
sandbox grants in place. Connector and registry changes made in another client
appear automatically through service notifications.

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
⌘F focuses search in Sandboxes, Connectors, and Audit. Escape clears a focused
search or closes focused event details. Standard macOS window controls and ⌘W
close a window without stopping the service; the menu-bar interface stays open.
⌘Q quits only the interface. Stopping the service is a separate, confirmed
action because it interrupts running sandboxes.

Dashboard pages and open sandbox configurations update live without a refresh
button. The interface clears actionable data when disconnected. Overlapping refreshes
are coalesced into a fresh post-action read, and canceled reads cannot restore
an old snapshot. A finite read that stops delivering complete frames times out;
established subscriptions remain idle without polling.

## Self-contained app bundle

On macOS, build a bundle containing the native interface and release-built
CLI/service helpers:

```sh
make -C clients/macos package
make -C clients/macos smoke
```

This produces `clients/macos/dist/LNS.app` and `LNS-macos.zip`. Copy the app to
your user-owned Applications directory, or run it from `dist`. On opening, the
app runs the bundled `lns service start` against the interface's socket. That command reuses an already-running service. If startup
fails, the app shows the error and offers **Start Service** to retry. It does not register
a login agent, replace a separately installed CLI, or stop an existing service.
An already-running service must have the same release version. Both service
startup and native client requests reject a different version without stopping
its workloads.

The helpers live in `LNS.app/Contents/Helpers/`. The macOS release installer
places the app in `~/Applications/LNS.app` and links `lns` and `lns-service` from
`~/.local/bin`. `APP_DIR` selects the parent app directory and `INSTALL_DIR`
selects the CLI directory. The CLI directory is remembered in the app's macOS
preferences so updates and uninstall find those links.

`lns update` downloads and checksum-verifies the release ZIP, verifies the app's
signature and Gatekeeper assessment, stages the complete bundle, closes the
interface, and stops the service before replacing it. It preserves existing
login startup and restores the previous app and CLI links if installation or
startup fails. Updating interrupts running sandboxes. `lns update --force`
reinstalls the current version; `--dry-run` remains read-only. An older
loose-binary installation migrates through the current release installer.

`lns uninstall` confirms, stops sandboxes and the service, removes login startup,
closes the interface, and removes the whole app plus CLI links that still point
to it. Data stays unless `--purge` was explicitly requested. Moving the app to
Trash manually does not perform this cleanup.

Packaging signs helpers before the enclosing app and verifies the signatures
before replacing an existing bundle. Each previous build is retained in a
printed `.lns-previous.*` directory under `dist` for recovery; remove those
development backups when no longer needed. `VERSION` defaults to the CLI crate
version and must agree with the packaged helper. `SIGN_IDENTITY` selects ad-hoc
signing (the default) or a configured Developer ID identity with hardened runtime
and timestamping. `make notarize` uses a `NOTARY_PROFILE` keychain profile to
submit the app, staple and validate Apple's ticket, and recreate the ZIP.
Updates are initiated explicitly with `lns update`.

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

Start the matching service build with `LNS_HEADLESS=1` to keep it from
opening the app automatically during isolated development. Use a dedicated
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

Confirmed management actions appear in a distinct notification at the bottom
right of the dashboard. It remains visible across page changes until dismissed
or replaced by the next action. Dismissing it does not repeat or undo the action.

Without an override, the app uses
`~/Library/Application Support/run.lns/service.sock`.

Local bundles are signed ad hoc unless `SIGN_IDENTITY` is set. Public release
jobs require Developer ID signing and successful notarization; development
artifacts do not pass the public installer's signature and Gatekeeper checks.

## Client contract

- `WatchDashboard` signals an initial refresh and later service writes. The app
  coalesces bursts and does not poll while idle.
- `ReadDashboard` returns a finite sequence: `DashboardBegin`, individual
  sandbox/approval/event/warning frames, then `DashboardEnd`. The client keeps
  every frame and publishes only a completed read. A large timeline is not one
  oversized frame; an interrupted read is an error, not an empty dashboard.
  `DashboardEnd` completes the request without waiting for a socket EOF.
- `InspectApprovalOffer` reads the offer a history row still holds.
- `PreviewSandbox` resolves a local or published definition with ordered extra
  mixins. `ReadRunConfiguration` reads the recorded configuration and current
  decisions through the service; the interface never reads run directories.
  Both return `SandboxConfiguration`, including source attribution, rule order,
  and connector variable names and file paths without credential values.
- `SaveRun` renders the document through the service. The interface writes it
  only to the user-selected new file and remembers the path after success.
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
- Account sign-in uses `BeginConnect`, then the service's `ConnectorAsks` fields
  or structured OAuth progress. `AnswerConnect` submits one round; OAuth status
  polls automatically after an explicit permission choice. Browser opening and
  cancellation go through the service. Closing the account form cancels its
  session, including a session whose first reply arrives after closing.
- Live sign-in rounds travel with approval snapshots as `connect` and
  `connect_seq`. Cards submit `begin_connect`, `answer_connect`, or
  `open_connect_browser` against their current token. The service keeps sign-in
  and the resulting grant bound to the original run. Cancel Sign-In dismisses
  the held request without recording a decision.
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

For release acceptance, verify a live approval through
the actual service and guest, simultaneous clients, reconnect after service
restart, connector expiry while typing, failed policy writes, VoiceOver,
keyboard navigation, Spaces, fullscreen, multiple displays, idle resource
usage, and installation/update behavior. Native platform checks cannot be
substituted by the Linux test result.

## Release credentials

The `release` GitHub environment supplies these secrets to the macOS release job:

| Secret | Value |
|---|---|
| `MACOS_CERTIFICATE_P12` | Base64-encoded Developer ID Application certificate and private key export |
| `MACOS_CERTIFICATE_PASSWORD` | Password for the P12 export |
| `MACOS_SIGN_IDENTITY` | Exact Developer ID Application signing identity |
| `MACOS_NOTARY_KEY_P8` | App Store Connect API private key, as its original multiline text |
| `MACOS_NOTARY_KEY_ID` | API key ID |
| `MACOS_NOTARY_ISSUER_ID` | API issuer ID |

The job imports credentials into a temporary keychain, builds the app with the
release's CLI and service, notarizes and staples it, runs the bundled-service
smoke check, and publishes `lns-<version>-darwin-aarch64.zip` with its SHA-256 to
GitHub Releases and the CDN. The latest manifest is published only after all
platform uploads succeed. Missing credentials, rejected notarization, or failed
smoke checks stop publication. The temporary keychain and credential files are
removed when the job step exits.

The service's hardened-runtime entitlement permits unsigned executable memory
for the Wasmtime component engine's generated code. The native app and CLI do
not receive that entitlement. Apple documents the distribution process in
[Customizing the notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).
