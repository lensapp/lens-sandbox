# OpenShell runtime spike

This branch replaces the executable `lns-supervisor` dependency graph with
OpenShell's network and process components. `lens-sandbox-core` is absent from
the workspace lockfile and the supervisor's dependency graph. The old supervisor
sources remain uncompiled as reference for the unfinished migration; this is
not a feature-complete replacement or a shipping build.

Baseline: LNS `aae90c1a`; OpenShell
`02b664bb0d978ac0baec9aaa0bf06a2a4f67e83d`.

## What runs

The real host service boots Vz with a static Linux-musl OpenShell-backed
supervisor. It receives LNS policy over authenticated WebSocket/vsock,
starts OpenShell's proxy outside a broker-owned workload network namespace,
and launches the workload with OpenShell's public process-hardening primitives. LNS still owns the VM,
OCI image, broker, run persistence, CLI, and approval UI.

The real-VM denial smoke test has passed: UID 65534, no inherited LNS relay
credentials, HTTP 403 through the proxy, and direct egress refused when proxy
environment variables are removed. A second scenario exercises a held HTTP
request and answers it through `lns approval answer`; that same request returned
HTTP 200 and the expected page body. Both scenarios passed together with exit 0
on 2026-09-11, with state retained at `/private/tmp/lns-os-e2e.v3OFm7`.

The host-side real-TCP regression also proves the proxy leaves a CONNECT socket
open until its LNS decision arrives. Component tests cover ordered decisions,
wildcards, binary scope, timeout, policy withdrawal, late decisions, and the
four host approval verdicts.

The Claude install scenario passed on 2026-09-11: the Node workload retained
`HOME=/home/node`, `USER=node`, and UID 1000, installed into an initially empty
npm volume over HTTPS, and printed `2.1.268 (Claude Code)` with exit 0. Evidence
is retained at `/private/tmp/lns-os-claude.ZTN8xO`, run
`080b47dccf1865d5d6f744b51b59c45e`. It uses no host bind or credentials and does
not exercise the interactive UI.

Pre-start package installation passed in a real Ubuntu VM on 2026-09-11:
root installed curl and CA certificates, then UID 1000 ran curl successfully.
The script could not read the broker environment, administer nftables, connect
directly out, or fetch a policy-denied destination. A failing first script
prevented later scripts and the workload, returning 125.

Scripts resolve their identities before execution, use `sh -e` with closed
stdin, stream prefixed output, and run with the same policy engine as the
workload. Their network namespace has only loopback and a pre-bound proxy
listener; proxy connections originate in the supervisor's namespace. Root keeps
only the package-management capability set (`0xfb`). The broker and supervisor
disable dumpability to protect their inherited relay environment.

## Important limits

- Credential/file payloads, CIDRs, explicit
  schemes, TLS termination directives, and HTTP inspection rules are refused.
- Outbound connections must be proxy-aware. Policy-filtered IPv4 DNS works over
  UDP/TCP; transparent TCP, general UDP, and raw-stream credential injection
  are not wired. Other DNS record types return no data for eligible names.
- PTY allocation, raw input, resize, and detach/reattach passed a live Node probe. Claude's theme
  selection and keyboard input worked, but its Ctrl-C exit test remains failing.
  CLI signal forwarding and signal-derived exit codes passed pipe and PTY probes.
  Published TCP ports, stop/start, and root broker exec passed targeted VM probes;
  per-request LNS audit events and full broker exec parity are not verified.
- Main workloads, pre-start scripts, and broker exec share an isolated network
  namespace with loopback and the proxy listener, but no external interface or
  route. Both root and non-root use the adapter launcher; no proxy UID exemption
  remains. Root retains only the package-management capability set (`0xfb`).
- An OpenShell-generated credential marker survives rotation in component
  tests, but LNS's existing literal markers do not resolve through that API.
  Stable marker recognition is essential; author-selected bytes are the current
  format contract, not a technical necessity. No document-format change is made.
- Upstream telemetry support is compiled through transitive default features;
  this is not evidence of telemetry transmission.

Unsupported policy payloads fail the run instead of silently dropping security
requirements. Do not use this branch for real secrets or untrusted production
workloads yet.

## Prepare and build

From this worktree's root on macOS Apple Silicon, with Zig and cargo-zigbuild:

```sh
sh spikes/openshell-network/prepare.sh
cargo build --locked -p lns-cli -p lns-service
cargo zigbuild --locked -p lns-supervisor -p lns-init -p lns-session-broker --target aarch64-unknown-linux-musl
sh crates/lns-cli/scripts/codesign-macos.sh target/debug/lns-service
sh crates/lns-cli/scripts/codesign-macos.sh target/debug/lns
sh spikes/openshell-network/e2e/run.sh
```

`prepare.sh` clones the pinned upstream into `target/openshell` and applies the
checked-in `openshell.patch`. Set `OPENSHELL_SOURCE` to an existing local clone
to avoid cloning from GitHub. Existing checkouts at another revision or with
unexpected edits are rejected. Do not run `cargo clean` without preparing again:
the source checkout currently lives under `target`.

The proxy patch touches `opa.rs` and `proxy.rs`: 70 added lines and three
removed. Guest DNS additionally uses a bounded upstream
lookup in `run.rs` and makes the existing resolver timeout constructor available
outside upstream tests. The combined patch is four files, +93/-4. The runtime
changes add the connection-approval hook and public proxy startup APIs,
including accepting a pre-bound listener and reading its address for tests.
OpenShell's process crate is unmodified. `verify.sh` checks that invariant.

The adapter's `launch` module owns clean child environments, namespace entry,
bounded-root capabilities, PTY creation, and managed-child registration. It uses
OpenShell's public privilege, hardening, Landlock/seccomp, and retained-terminal
components. The broker retains the namespace and a sealed in-memory supervisor
snapshot, passing reserved descriptors to the privileged launcher. Exec never
reloads that launcher from workload-writable paths. Init mounts `/.lens/bin`
read-only from the trusted lower stack on each boot, covering retained decoys
and clearing parent symlinks before mounting. Distinct real/effective
launcher UIDs prevent a workload UID from matching all privileged process IDs
across exec; child identities are normalized before capability restriction.
Port-forward connections enter the workload namespace before connecting;
the proxy address uses a loopback alias so localhost remains available.
The old UID-exemption firewall and separate script namespace implementation
have been removed.

This reduces upstream patch maintenance, not the amount of security-sensitive
code we own. The adapter explicitly selects raw TLS tunnels; TLS inspection
remains unimplemented.

The E2E script uses a new short `/private/tmp/lns-os-e2e.*` directory for its
service socket and cache; it does not replace your installed service. It stops
its own service on exit and retains logs and run files for inspection. It needs
network access to obtain Alpine and the kernel, and the approval scenario fetches
the public `http://example.com/` page. Runs have explicit deadlines.

For the locally installed build helper in this worktree, substitute
`target/build-tools/bin/cargo-zigbuild zigbuild` for `cargo zigbuild`, and set
`CARGO_ZIGBUILD_CACHE_DIR` and `ZIG_GLOBAL_CACHE_DIR` to writable cache directories
under this worktree's `target`.

## Component verification

### Current DNS direction

DNS does not raise approval prompts. The adapter's `Gate::dns_eligible` checks
whether the active policy allows at least one destination port for the queried
name, preserving TCP-before-HTTP rule precedence and per-port shadowing. Unknown
names are refused. DNS eligibility ignores binary scope by the spike's accepted
decision; connection authorization still enforces it. Policy quarantine and
withdrawal invalidate eligibility. The result is a policy snapshot, not a grant
to resolve later without revalidation.

The guest now has namespace-local UDP/TCP DNS listeners at its configured IPv4
resolver addresses. Allowed A queries use OpenShell's upstream resolver outside
the workload namespace, with bounded concurrency and timeouts. Responses contain
only IPv4 addresses, with zero TTL, and policy is rechecked after resolution.
Unknown names return NXDOMAIN without an upstream lookup or approval prompt;
eligible non-A queries return no data. Denials enter the host audit stream.
No DNS-only permissions or document fields are introduced. These are real DNS
addresses, not synthetic destinations for transparent connection interception;
transparent TCP and its destination correlation remain unwired.

This slice requires an IPv4 resolver configuration and uses its first resolver
without fallback. OpenShell follows CNAMEs for an eligible original name; alias
targets are not independently checked against LNS policy. The denial audit
queue is bounded and can drop events under overload.

The real-VM probe passed on 2026-09-11 for default resolver calls, DNS over TCP,
unknown-name refusal, AAAA no-data responses, root broker exec, and host audit
delivery. Run it against an isolated service with:

```sh
sh spikes/openshell-network/e2e/terminal.sh ../../../crates/e2e-tests/probes/openshell-guest-dns.exp
```

One-shot proxy approvals do not currently make a name eligible for standalone
DNS; persistent connection decisions that produce allow rules can do so after
reload. This is narrower than main's temporary approved-host tracking.

### Superseded approval-gated DNS experiment

The following experiment is retained as evidence, not the selected architecture;
it will not be wired into the approval UI or VM launcher.

```sh
cargo run --locked --manifest-path spikes/openshell-network/Cargo.toml --example dns-boundary
```

The probe lives in `crates/e2e-tests/probes/openshell-dns.rs`. It uses OpenShell's
real DNS client, a loopback-only LNS approval boundary, and a loopback fake
external resolver. No public DNS is contacted. The expected packet counts are
zero before approval, zero after denial, and one after approval. It also proves
the existing two-second exchange budget expires before a three-second approval,
whereas an explicitly supplied fifteen-second client budget completes.

`dns::resolve` waits up to sixty seconds for permission, then gives the upstream
operation its own two-second budget. Approval loss, denial, and expiry do not
start the upstream operation. There is no alternate-resolver fallback and no
connection-policy mutation. The caller must scope the approval to the exact
query/run and revalidate policy; this primitive does not own policy or grants.

This is a boundary experiment, not a runnable sandbox DNS proxy. Its packet
forwarder and response fixtures are probe-only code, not a production DNS parser.
Approvals come from the harness, not the UI. DNS eligibility, request deduplication,
revocation during a pending approval, authenticated vsock transport, resolver
configuration, CNAME handling across permissions, and VM capture remain unwired.
Normal OpenShell runtime timeout and DNS behavior are unchanged. Do not interpret
this probe as preventing DNS leakage in the current VM launcher.

### Existing component and VM probes

```sh
sh spikes/openshell-network/verify.sh
sh spikes/openshell-network/e2e/claude.sh
sh spikes/openshell-network/e2e/scripts.sh
sh spikes/openshell-network/e2e/terminal.sh scripts-terminal.exp
sh spikes/openshell-network/e2e/terminal.sh lifecycle.exp
```

The socket test requires local TCP binding. On macOS it explicitly uses
OpenShell's endpoint-only identity mode; that test does not claim Linux process
attribution. The guest smoke test does not enable that relaxation.

The original `admission-probe` remains a component demo, not a sandbox launcher.
Its aarch64 and x86_64 musl link checks passed before runtime integration.

## Red evidence and gate status

Current implementation progress is tracked in [PARITY.md](PARITY.md).
Supervisor seccomp and primary-workload orphan reaping are now wired and passed
a real VM probe. The earlier macOS `xcrun_db` harness failure described below
was fixed by isolating tools' temporary files inside the harness's cleanup root;
the unchanged env-parity suite now passes all 20 checks.
The final lifecycle slice passes workspace lint and complexity, Linux-musl
strict clippy, and `verify.sh`. Affected coverage skips the uncommitted diff.
Full coverage stops in the unchanged artifact invalid-UTF-8 filename fixture:
macOS rejects creation with `Illegal byte sequence` outside the tool sandbox
(and `Operation not permitted` inside it). Claude's onboarding accepts input, but
its Ctrl-C exit probe still hangs and is not certified.

- Pre-start identity and ordering tests failed with `pre-start scripts not
  wired`; the failure-sequencing test observed no calls instead of the first
  script. Root capability testing failed with `0` instead of `251`. Broker
  protection testing incorrectly returned success for a denied `prctl`.
  These tests now pass. The real install initially reached the workload but
  failed on test-fixture shell quoting; correcting the fixture completed the run.
- The pre-start slice used red-first tests and the green verification ladder.
  Its initial full lint attempt stopped at the macOS `xcrun_db` shell-harness
  failure, subsequently repaired in the lifecycle slice. No commit or push was made.

- Before the proxy hook: `proxy completed before approval: ... 403 Forbidden`.
- Before policy-update wakeup: `policy update must wake the held request:
  Elapsed(())`; the real guest also timed out despite a saved allow decision.
- Duplicate host policy frames originally advanced OpenShell's generation and
  invalidated the newly admitted request. A regression pins identical-policy
  deduplication while still permitting recovery from fail-closed quarantine.
- The first allowed HTTP request reached an undefined Rego middleware query;
  its regression failed with `not a valid rule path` before supplying the empty
  chain that raw forwarding requires.
- The first VM fixture explicitly named a user absent from Alpine and failed
  before supervisor execution. The fixture now names UID/GID 65534.
- Workload identity regressions failed with missing `/home/node` and an empty
  OpenShell override map before preserving resolved and explicitly declared
  HOME/USER. Internal relay markers are still scrubbed.
- CONNECT's credential-guard query failed with `not a valid rule path` before
  adding the empty guard list appropriate to this credential-free adapter.
- Raw HTTPS selected `Auto` instead of `Skip`; the live npm install reset its
  connection until the adapter supplied a named raw TLS route. A one-shot
  approval regression also failed because OpenShell discarded its route name.
- The first Claude fixture omitted `sh -c` and failed with `test: extra argument
  '&&'`; the corrected fixture subsequently exercised the real HTTPS failure.
- E2E mutation testing was skipped; actual failing VM runs and red-to-green
  component tests provide the sensitivity evidence. Full migration verification
  remains outstanding.
