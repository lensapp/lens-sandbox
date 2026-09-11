# Current LNS parity

Scope: preserve the current LNS CLI, document grammar, policy semantics, and
microVM architecture while replacing the guest supervisor's core dependency.
OpenShell gateway, SSH, cloud metadata, and extension features are not goals.

## 1. Runtime and confinement

- [x] Apply supervisor startup hardening before untrusted execution.
- [x] Forward signals and preserve signal-derived exit codes in pipe and PTY sessions.
- [x] Verify raw input, resize, detach/reattach, final output, and pipe mode with a real workload.
- [ ] Resolve Claude's terminal-exit hang after it receives Ctrl-C.
- [x] Reap primary-workload and script orphans without stealing managed child exit statuses.
- [x] Cover broker-exec orphan cleanup as part of exec confinement integration.
- [x] Give primary workloads, scripts, and broker exec consistent confinement.
- [x] Support bounded root workloads without the proxy UID exemption.
- [x] Verify published TCP ports and stop/start with the resulting namespace model.

## 2. Policy and integration

- [x] No-prompt, sandbox-wide IPv4 DNS eligibility with UDP/TCP guest listeners and denial audit.
- [ ] Transparent TCP and temporary approved-host DNS tracking.
- [ ] CIDR, scheme, HTTP rules, TLS inspection, and binary-scoped policy parity.
- [ ] Existing credential placeholders, injections, refresh, and revocation.
- [ ] Runtime connector files and their ownership/removal semantics.
- [ ] Connection/request audit events and approval attribution without secret leakage.
- [ ] Reconnect and policy-update failure behavior.

## 3. Verification and handoff

- [ ] Characterize current behavior with lower-layer tests and representative VM scenarios.
- [ ] Recheck images, binds, volumes, filesets, tools, and existing published recipes.
- [ ] Reproducible upstream patch, clean builds, and obsolete-source removal.
- [ ] Full lint, complexity, affected coverage, and real-VM regression checks.

## Existing evidence

Destination admission, held approvals, raw HTTPS, non-root process execution,
basic terminal input/resize, and bounded root pre-start package installation
have passed targeted VM tests. The lifecycle VM suite additionally verified
CLI-delivered SIGINT with exit 130 in pipe and PTY modes, supervisor seccomp,
and actual orphan disappearance. The expanded terminal suite passed reattach.

The old macOS env-parity failure (`xcrun_db`) reproduced with 18 passes and two
failures; giving test-lib's tools an owned TMPDIR made the unchanged suite pass
all 20 checks. Workspace lint and complexity, Linux-musl strict clippy, all 22
supervisor unit tests, and `verify.sh` pass. Affected coverage skips because
the spike is uncommitted. Explicit full coverage stops in unchanged
`lns-artifact::walk::tests::a_non_utf8_file_name_is_refused_rather_than_lossily_packed`:
creating the fixture fails with `Operation not permitted` inside the tool
sandbox and `Illegal byte sequence` outside it. No test or assertion was skipped.
Focused coverage measured lifecycle, reaper, pre-start, terminal, and workload
environment modules at 100%; the broker's protection seam is also at 100%.
Their three test binaries pass environment parity. The existing binary-entry
coverage exemption now names `openshell.rs` instead of the inactive `main.rs`;
Linux guest wiring remains verified by the targeted VM probes, not host coverage.
The final fmt, locked workspace lint, complexity, and two-crate coverage ladder
passes. This focused pass does not supersede the full-workspace failure above.

Before patch reduction, the root-install regression passed, including denied
destinations and script failure preventing workload startup. Two earlier runs
failed after policy admission while resolving the Ubuntu mirror; diagnostic
lookups and the subsequent normal run succeeded. The intermittent failure's
cause is unresolved; no DNS warmup or relaxed policy was retained.

## Red-first evidence

- Guest DNS: the first wire-response test failed with `denied DNS needs a
  failure response`; the resolver-config test failed with an empty address list
  instead of the configured IPv4 resolvers. Both pass after implementation.
  Technical tests cover denial without upstream resolution, post-resolution
  policy withdrawal, non-A no-data behavior, and malformed/multiple questions.
  `openshell-guest-dns.exp` passed in a real VM: default `resolve4` and `lookup`,
  TCP DNS, unknown-name NXDOMAIN, AAAA no-data, the same checks via root exec,
  and delivery of the unique denied hostname to host audit. Namespace-local
  listeners use the existing resolver addresses; upstream resolution happens
  outside the workload namespace through OpenShell's bounded resolver API.
  Transparent TCP remains unwired, and no DNS approval path was introduced.
  Removing the post-resolution eligibility check made the withdrawal test fail
  with one answer instead of zero; restoring it restored all four wire tests.
  The existing root main/exec confinement, root apt/curl installation, script
  failure, restart, and published-port VM regressions also pass with guest DNS
  enabled. The combined upstream patch remains four files, +93/-4; its reverse
  application check passes. The 33 adapter unit tests and 24 supervisor unit
  tests plus matcher parity pass. No separate expensive VM mutation was run.
  Workspace lint/complexity and Linux-musl strict clippy pass; automatic affected
  coverage skips the uncommitted spike and does not supersede the failures below.
- No-prompt DNS eligibility: the initial test failed with `allowed name must be
  DNS eligible: example.com`. The adapter now keeps its validated policy with
  the corresponding engine generation and computes sandbox-wide eligibility
  from ordered TCP/HTTP rules. Tests cover wildcard apex/nested names, distinct
  binary-scoped connection refusal, port-specific shadowing, invalid query names,
  unknown-name refusal without approval, quarantine, reload, and withdrawal.
  At this stage it was a policy-layer slice, not live guest DNS; the subsequent
  guest slice above adds listeners and denial audit. One-shot approved-host
  tracking and transparent response correlation remain outstanding.
  The earlier separate-DNS-approval experiment below is superseded
  and is not selected for UI or runtime integration. Upstream patch unchanged.
  A mutation that skipped a leading deny made the ordering test return true
  instead of false; restoring the code restored the passing test. The spike's
  29 unit tests and five integration tests pass, along with strict spike clippy
  and workspace lint/complexity. Affected coverage skips the uncommitted diff;
  the previously recorded coverage failures remain unresolved. No VM DNS probe
  is claimed for this policy-only slice.
- DNS approval boundary: `dns::tests::denial_never_calls_the_external_resolver`
  failed with `DNS escaped before permission` (one call instead of zero), and
  the delayed-approval test completed before approval. Both now pass after
  separating the sixty-second approval wait from the two-second resolver budget.
  Five technical tests cover denial, delayed approval, unavailable/expired
  approval, resolver failure/timeout without fallback, and unchanged connection
  authorization. The loopback Layer 1 `dns-boundary` example passes with
  OpenShell's actual resolver: its stock two-second budget expires before the
  three-second harness decision; an explicit fifteen-second budget permits the
  decision, denial emits no upstream packet, approval emits one and resolves
  the expected fixture address. No public DNS was contacted. A small public
  upstream lookup entry point plus availability of the existing timeout
  constructor adds +23/-1 to the prior patch: combined +93/-4 across four files.
  This is not wired into VM DNS, host approvals, or policy eligibility yet.
  No E2E mutation was added; the lower-layer red tests and stock-timeout
  characterization provide the initial sensitivity evidence.
  The spike suite passes (25 unit tests and five integration tests), as do
  spike strict clippy and workspace lint/complexity. Affected coverage skips
  the uncommitted diff; this run does not supersede the coverage failures below.
- Restart and published ports: the init test failed with `trusted runtime bin
  must cover the retained writable copy`. Init now mounts `/.lens/bin` from a
  read-only, executable lower-stack view on each boot, clearing parent symlinks
  before mounting. Runtime binaries can no longer be patched by a root workload;
  workload paths outside that reserved directory remain writable. Mount failures
  abort boot. The VM probe then failed with `listen EADDRNOTAVAIL` for localhost:
  assigning the proxy address had replaced loopback's primary address. The
  broker's interface-plan test failed on `lo` instead of `lo:lns`; proxy setup now
  uses that alias and retains `127.0.0.1`.
  `crates/e2e-tests/probes/openshell-restart.exp` passes published TCP before/after
  restart, durable rootfs state, rejected runtime-binary writes, sealed exec after
  replacing `/.lens` with a decoy symlink, and trusted main/exec startup after
  restart. This confirms the init/broker/service boundary; no separate expensive
  E2E mutation was run. The upstream patch remains two files, +70/-3.
  Root main/exec confinement and the root apt/curl plus script-failure probes
  also pass with the new binaries. Focused instrumented tests and environment
  parity pass (init 220, broker 126, supervisor 23 plus matcher parity, session
  13). Broker `isolation.rs` is 100% (81/81). The expanded focused coverage gate
  is red: init `mount.rs` 94.91% (2986/3146), broker `session.rs` 94.24%
  (327/347), session `lib.rs` 95.92% (94/98). This is not a green handoff; no
  exemptions or weakened assertions were added. Workspace lint/complexity and
  Linux-musl strict clippy pass; automatic affected coverage still skips the
  uncommitted spike. Remaining work includes these coverage failures, broader
  policy/credential parity, and Claude's terminal-exit hang.
- Unified confinement: the launcher test failed because UID 1000 could launch
  without isolation; broker routing returned `Direct` instead of `Primary`;
  the snapshot-seal test received mask 0 instead of 15; the forwarding test
  observed only `connect`, not `enter` then `connect`; launcher identity tests
  found UID 0 matched all privileged IDs. Each now passes after the associated
  boundary was implemented. A lifecycle probe also found the sealed-memory
  launch no longer carried the supervisor process name; the naming-port test
  failed on a swallowed error before naming was wired into startup.
- The new `crates/e2e-tests/probes/openshell-confinement.exp` exercises root main
  execution and pipe/PTY exec, capability and seccomp bounds, environment
  scrubbing, orphan reaping, direct-egress refusal, and allowed/denied HTTP.
  The final probe passes, including privileged-launcher identity and environment
  isolation assertions. The existing terminal probe passes raw input, resize,
  detach/reattach, output completion, and pipes. The final lifecycle and script
  probes pass signal exit 130, supervisor hardening, orphan reaping, root apt
  installation, network denials, and script-failure refusal.
- Focused instrumented tests and environment parity pass (broker 124 tests,
  supervisor 23 plus matcher parity, session 13). Broker `isolation.rs` measures
  100% (59/59), and supervisor `lifecycle.rs` measures 100% (55/55). The focused
  coverage gate still fails twice on broker `session.rs` (94.24%, 327/347) and
  session `lib.rs` (95.92%, 94/98); reported misses include existing test-source
  expressions and the `identity_env` closure. No coverage exemption was added.
  This is not a fully green handoff. Linux-musl strict clippy passes; automatic
  affected coverage skips the empty committed diff. No E2E mutation run was
  added; lower-layer red tests and observed VM failures supply sensitivity
  evidence. The subsequent restart slice above adds published-port and retained
  launcher-path protection evidence.
- Follow-up root-install diagnosis: runs `b7ddb35f4217c24ddd2867cb6a29a686`
  and `d45fb255bd7d82980ac1a5fdb5cbecf9` failed before any successful package
  download. The proxy denial channel reported `DNS resolution failed` after
  policy admission. A supervisor-side probe reported `Network unreachable`;
  both guest consoles showed `udhcpc: no lease, failing` before supervisor
  startup. The broker continued with fallback DNS despite DHCP failure.
  The host lease file contained all 253 usable addresses in the VM subnet;
  address-pool exhaustion is a hypothesis, not established by that file alone.
  Temporary denial logging and DNS probes were removed. No network policy,
  host networking, or upstream patch was changed to bypass the failure.
- After the last manual LNS VM stopped, the next isolated boot recreated the
  transient macOS DHCP service and obtained a lease. Run
  `27e7ad545b1616015c1fe7c4b43b9946` passed root package installation, direct
  egress refusal, policy-denied HTTP, allowed HTTP, and non-root workload
  startup using the unchanged reduced-patch binary. The companion failing
  script test also passed; `e2e/scripts.sh` exited zero. No privileged recovery
  command succeeded and no host lease file was modified. This establishes
  recovery for this run, not the cause of the earlier DHCP outage or a durable
  fix for it.
- Patch reduction: the adapter launcher initially accepted unisolated root,
  retained internal environment keys, and had a zero capability mask instead
  of 251; its firewall test failed with `missing policy drop`. All four pass
  after moving these responsibilities into the adapter. The process crate now
  has no upstream diff; only two networking files remain patched (+70/-3).
- With the reduced patch, the root script passed its UID, HOME, cwd, closed
  stdin, token scrubbing, broker-environment denial, and capability checks;
  nftables administration remained denied. Apt then received proxy 403s in two
  runs, after successful binary attribution and an explicit policy allow.
  Root package installation is not re-certified by this change; its earlier
  intermittent destination-resolution failure remains unresolved.
- The reduced-patch binary passes the real-VM terminal and lifecycle probes:
  raw input, resize, detach/reattach, final output, piped I/O, signal exit 130,
  supervisor seccomp, and orphan reaping. The component verification suite,
  Linux-musl strict clippy, workspace fmt, locked lint, and complexity pass.
  Affected coverage skips because the committed diff is empty; this is not
  coverage evidence for the uncommitted patch reduction.

- `lifecycle.rs`: signal status returned 125 instead of 130; hardening was skipped
  and incorrectly succeeded on failure; signal forwarding made no group call.
  Group 1 also reached the sender before being rejected to prevent `kill(-1)`.
- `reaper.rs`: the reaped list was empty instead of containing orphan PIDs.
- `scripts/env-parity.test.sh`: both cleanup assertions found `xcrun_db`.

The unit seams now exercise these cases without real process or filesystem I/O.
Real wiring is exercised by `e2e/lifecycle.exp` and `e2e/terminal.exp` through
`sh spikes/openshell-network/e2e/terminal.sh <filename>`.

This checklist records implementation progress, not changes to the settled
document or CLI specifications. Unchecked items are not claimed as complete.
