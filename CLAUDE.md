## Product Vision

LNS is a local desktop app for running AI agents, commands, OCI images, and other untrusted workloads inside a microVM. Tagline: "The sandbox you'll actually leave running." One-liner: "Run AI agents, commands, and OCI images locally. Control access into and out of the sandbox."

- **Start here:** `docs/README.md` — the first-party user documentation index.
- **Product language:** the Product Vision above and `docs/` are the source of truth for terminology and framing; keep naming consistent with them. `docs/` is user-facing documentation only — no internal sales/marketing material lives in this repo.
- **Concepts:** approvals drive policy authoring; a per-run decisions file (`~/.lns/runs/<RUN>/decisions.yaml`, a `kind: mixin` document created empty with the run; a destination no rule decides is asked about) holds the network rules, and `lns sandbox save` writes a run out as a document to keep; `Vz` on macOS / `KVM` on Linux is the only runtime; real secrets stay outside the workload.
- **Target format:** `docs/sandbox-spec.md` is the normative specification for the `lns.run/v1` document format and the decisions behind it. **The code does not implement all of it** — see [Transitional mode](#transitional-mode) before you touch a document-format surface.
- **Sibling product:** Lens Agents is the centrally managed counterpart for IT teams. Same policy model.

Before proposing new features or architecture, consider whether they preserve the core principles: **a sandbox you don't turn off**, **disposable, never leaky** (a run's state never escapes the sandbox; stopped runs persist until removed, and one `prune` sweeps them all away), **no system dependencies** (the user runs one binary; no apt/brew preflight, no privileged installer), **policy you run into, not write**, **the working directory only roots relative paths**, **real secrets stay outside the workload**. A small user-launched background service (the tray-resident `lns-service`, started by `lns service start` and stoppable via the tray Quit menu or `lns service stop`) is part of "a sandbox you don't turn off" — not a daemon in the apt/launchd sense.

## Transitional mode

**The document format has an agreed target that the code has not reached.** `docs/sandbox-spec.md` states that target and states it as settled. This section is the contributor rule for the gap between them. It is temporary and shrinks as the gap closes.

Read this before changing anything that parses, validates, publishes, resolves, or merges an `lns.yaml` or a run's decisions file.

### Which document wins

| Question | Authority |
|---|---|
| What *should* the format be? | `docs/sandbox-spec.md`. It is the decision, not a proposal. |
| What *should* the CLI surface be? | `docs/cli-spec.md`. Same standing, for the command grammar, output, exit codes, and prompts. |
| What does `lns` accept *today*? | The code, and the guides in `docs/` (including `docs/cli-reference.md`) that describe it. |

Never edit the specification to match the code. A divergence is work to be done, not a doc bug. If the specification is genuinely wrong — the code encodes a constraint the spec did not know about — say so and change the spec deliberately, as a decision, in its own commit.

### The rules

1. **Move toward the target, never away.** A change may leave a divergence in place. It may not deepen one, and it may not add a new field, flag, or file shape that the specification does not describe.
2. **No compatibility shims.** The product is pre-1.0. Closing a gap is a breaking, unversioned change: change the format, change every caller and test, delete the old spelling. Do not carry both.
3. **The guides describe what ships.** Do not rewrite a user guide to describe unimplemented behaviour, and do not annotate one with what the target says instead — a guide answers "how do I use this today".
4. **Closing a gap is behaviour work, so it starts with a failing test** in the layer that owns it (see [Workflow](#workflow)). A format change with no red test first is not done.
5. **Read the code for what ships.** This repository does not carry a list of where the two disagree, so do not add one and do not trust a stale summary: before changing a format surface, read the code that owns it and read the section of the specification that decides it.

## Project Overview

Monorepo. A Cargo workspace (production crates, two operator tools, two test/coverage infra crates) and one shell-script package; `Cargo.toml` `members` is the authoritative list.

| Package | Purpose |
|---------|---------|
| `crates/lns-cli` | The `lns` developer CLI — thin clap-driven IPC client that drives the daemon. The shipping artifact. |
| `crates/lns-service` | Tray-resident background service. Owns the microVM lifecycle, OCI ingest, content / layer caches, supervisor relay, and audit-chain writer; exposes a local Unix-socket IPC. |
| `crates/lns-ipc` | Shared `Request`/`Response` types and wire-format codec for the lns-cli ↔ lns-service contract. |
| `crates/lns-spec` | The `lns.run/v1` document grammar — the shared definitions every `kind` is built from (`docs/sandbox-spec.md`). |
| `crates/lns-policy` | Policy schema — file-format types shared between lns-service (enforcement) and lns-cli (run-summary introspection). |
| `crates/lns-placement` | Where a written file lands against one mount — the rule the document check, the pre-boot check, the grant check, and the guest all decide by. No dependencies: lns-init carries it into the initramfs. |
| `crates/lns-artifact` | Typed OCI artifact model — spec types, parsers, and producer-side validation. |
| `crates/lns-audit` | Audit-timeline reader — merges per-run logs and the connection ledger into one chronological view. |
| `crates/lns-ocsf` | OCSF v1.7.0 event builders for the audit logs. |
| `crates/lns-init` | Static-musl PID-1 for the guest microVM. Mounts composefs/overlay then `fexecve`'s `lns-session-broker`. |
| `crates/lns-session` | Wire-protocol types (postcard) for the host ↔ guest session channel. Shared by `lns-service` and `lns-session-broker`. |
| `crates/lns-session-broker` | Static-musl guest-side session host. `lns-init` execs into it; it owns PTY allocation, per-session workload forks, and vsock framing. |
| `crates/lns-supervisor` | Static-musl in-guest supervisor built on `lens-sandbox-core`. Embedded into `lns-service` and run inside the microVM; owns the agent process lifecycle, nftables network lockdown, privilege drop, and the vsock relay client. |
| `crates/bump-kernel` | Operator tooling for managing the kernel pin (`crates/lns-service/kernels.toml`). |
| `crates/bump-mise` | Operator tooling for the pinned mise tool-provisioning engine and its registry snapshot. |
| `scripts/lns-install` | Installer shell script published to `get.lns.run`. |

## Conventions

- **Git**: Conventional commits (`feat:`, `fix:`, `chore:`, `refactor:`, `test:`, `ci:`). No Co-Authored-By trailer. No "Generated with Claude Code" footer in PR descriptions.
- **Comments**: A comment is a code smell, not a feature — comments rot, code keeps moving, and stale comments are worse than none. Default is zero. Before adding any comment, first try to make the code carry the meaning: rename the binding, split the function, encode the invariant in a type, replace a literal with a named constant. Only when no refactor can convey the WHY does a comment go in, and then it is **one sentence, single line, no paragraph**. If it doesn't fit one sentence, the code is wrong, not the comment. The only categorical exceptions are `// SAFETY: <why this unsafe is sound>` on every `unsafe` block (clippy enforces) and `// no-op: <one-line reason>` above intentionally-empty Cucumber step defs. No "what is this" doc comments anywhere — `///` on internal items follows the same one-sentence-max bar; `pub` items on cross-crate API surfaces (`lns-ipc`, `lns-session`, exported traits) may carry one-sentence contract docs where the signature alone can't. No section-divider banners (`// =====`, `// ─────`). No `# Arguments` / `# Returns` boilerplate. No step-by-step narration inside function bodies — if a body needs a story, extract named helpers.
- **No prototype shortcuts**: avoid `unwrap()` outside tests, `unsafe` blocks without justification, suppressed lints, or papered-over errors. Fix things, don't paper over them.
- **Logging**: always go through `crate::log` — never call `tracing::*!` macros directly. The five entry points are `log::error!`, `log::warn!`, `log::info!(verb, msg)`, `log::debug!`, and (re-exported on demand) `log::trace!`. error/warn/info render with cargo styling; debug/trace land on the developer trace stream. `log::TARGET` is the only target string our code emits.

## Workflow

1. Identify which layer the new behavior belongs to (see [Test layers](#test-layers)):
   - User-visible behavior of a crate → **Layer 2** Gherkin (`crates/<prod>/tests/behaviours/*.feature` — mocks for cross-crate deps, no real I/O).
   - Internal corner case / invariant not expressible as Gherkin → **Layer 3** (`#[cfg(test)] mod tests` inline; injected ports for FS / process / clock / network).
   - End-to-end wiring confirmation across crates through real binaries → **Layer 1** (`crates/e2e-tests/`; not measured for coverage).
2. Write the failing test in the right layer's location. For Layer 2, that's a `.feature` file plus a step definition in the crate's `tests/behaviours/steps/`. For Layer 3, a `#[test]` next to the module under test.
3. Watch it fail.
4. Implement the minimum production code to make it pass. If Layer 2 needs to mock a dep that doesn't have a `pub` trait surface yet, extract the seam first (see [Extracting library seams](#extracting-library-seams)).
5. From the workspace root, run the full verification gate (below) before considering work done.

## Verification gate

The **local pre-push gate** is `make lint && make complexity && make coverage-affected` — three targets defined in the top-level Makefile, run serially by `scripts/hooks/pre-push` (`LNS_PREPUSH_FULL_COVERAGE=1` swaps the last step for full `make coverage`). The **CI required suite** runs the same three as parallel jobs (with `CARGO_LOCKED=--locked` for `lint` / `test`) plus `make test`, `make e2e`, the kernel pin drift check, and a path-gated `check-release-build` (real cross-builds + codesign). The Makefile is the single source of truth for both.

- `make lint` — `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings`. Also the gate's compile signal (clippy is a strict superset of `cargo check`).
- `make complexity` — per-crate `cargo clippy -- -D clippy::cognitive_complexity` (per-crate because workspace feature unification disagrees with per-crate runs). For genuinely-branchy functions, use `#[allow(clippy::cognitive_complexity)]` with a one-line reason.
- `make coverage` — compiles and runs all tests **instrumented** in `target/llvm-cov-target/`, then enforces every file at 100% line coverage unless listed in `scripts/coverage-floor.sh`'s IGNORES table. Test failures surface here. See [Per-file coverage gate](#per-file-coverage-gate) below.

`make install-hooks` wires the gate into pre-push. Bypass: `git push --no-verify`.

### Build-cache rules the gate depends on

Three separate cargo target dirs, because cargo fingerprints the flags each step uses. Mixing them makes every step recompile what the previous one just built.

- `target/` — `make dev`, `make lint`, `make test`, raw cargo, rust-analyzer. These agree on flags, so switching between them is free. Do not add `CARGO_INCREMENTAL` or extra `RUSTFLAGS` to `lint` or `test`.
- `target/complexity/` — `make complexity` only. It passes different clippy args than `lint`, so it gets its own dir.
- `target/llvm-cov-target/` — `make coverage` only. It is instrumented.

`crates/lns-service/build.rs` embeds the guest binaries (lns-init, session-broker, supervisor, static-nft) in **release** builds and skips them in **debug** builds. That keeps one fingerprint across every debug caller, so no gate step needs to export `LNS_*_BIN=skip`. Set `LNS_<NAME>_BIN=<path>` to point a build at a pre-built guest binary; a shipping artifact must be a release build.

`make coverage` clears only the profraw counters between runs. It falls back to a full artifact clean when the toolchain or any manifest changes, because those shift the artifact hashes and leave superseded binaries behind for `llvm-cov report` to find. The stamp that decides this is `.gate/coverage-toolchain-stamp`, which also hashes every workspace `Cargo.toml`.

### Environment parity

**A test result must never depend on the host.** Not on uid, umask, TZ, locale, `HOME`, `TMPDIR`, or proxy variables. A test that inverts on a root box is a test nobody can read, and one that silently stops covering its branch is worse — it stays green while pinning nothing.

`make coverage` enforces this. After the test run it invokes `make parity`, which re-runs the test binaries it has just built — no recompilation, about 15 seconds — under a deliberately different environment: an unprivileged uid where the first pass was root, plus a fresh `HOME` and `TMPDIR`, `TZ=LNS-14`, `LC_ALL=C`, `umask 077`, and a dead HTTP proxy with loopback exempted. The first pass is the ordinary test run, which aborts the recipe on failure; parity judges the second. A binary that fails only there fails the gate, named. A pass that runs nothing fails too — silence must never read as agreement.

Three things to know before you run it: it needs `jq` and, to drop privileges, `setpriv`; when it runs as root it adds `o+x` (never `o+r`) to any ancestor of the workspace that lacks it, so the unprivileged pass can reach the binaries, and it hands every bit back on exit; and `LNS_ENV_PARITY=0` skips it. `scripts/env-parity.test.sh` covers the harness.

CI covers the other direction: the `coverage-as-root` job runs the same gate in a root container, because the runner is never root. Between the two, every PR exercises both uids.

Never inject a failure by removing a permission bit — root ignores it. Take the failure through a seam instead: a parameter for the writer, the remover, or the tmp path; a port method; or an error the kernel enforces for everyone, such as `EEXIST` from `O_EXCL` or `ENOTDIR` from a path component that is a regular file.

### Gate telemetry

`scripts/gate-timing.sh` records one row per gate step — the pre-push hook wraps each `make` call — plus the affected-crates verdict, in `.gate/timings.tsv`. That path is outside `target/`, so `cargo clean` keeps the history. Read it with `make gate-report`: runs, failures, and min/median/max seconds per step, then a count per coverage verdict (`__NONE__`, `__FULL__`, or the exact crate list). Set `LNS_GATE_TIMING=0` to stop recording. `scripts/gate-timing.test.sh` covers the script.

Outside the gate: `make build` (shipping artifacts), `make test` (uninstrumented — coverage already runs the same tests instrumented), `make e2e` (real binaries — runs in CI's test job, manual locally). The live-microVM interactive-shell smoke test (`crates/lns-cli/tests/smoke/interactive-shell.exp`) is run manually via `expect -f` when touching `lns run -it` plumbing — no Makefile wrapper.

## Test layers

The codebase follows a three-layer test pyramid. The layers are distinct in **what** they prove, **where they live**, and **whether they count for coverage**.

| Layer | What it proves | Location | Side effects | Counts for coverage? |
|-------|----------------|----------|--------------|----------------------|
| **1. E2E** | The system actually works end-to-end through real binaries and real I/O. | `crates/e2e-tests/` — a dedicated workspace member. Gherkin features live next to the production crates they describe (`crates/lns-cli/e2e/*.feature`, `crates/lns-service/e2e/*.feature`); the cucumber glue lives in `crates/e2e-tests/` and globs them in. | **Allowed.** Spawns real `lns` / `lns-service` subprocesses, writes real files, runs real daemons. | **No.** E2E confirms wiring; coverage is measured at the lower layers. |
| **2. Behavioural unit (Gherkin)** | A crate's user-visible behavior, from the outside, with cross-crate dependencies mocked. | `crates/<prod>/tests/*.rs` (Rust integration-test convention) with `.feature` files under `crates/<prod>/tests/behaviours/`. Each production crate's behavioural tests can only see its own `pub` API. | **Forbidden.** No FS writes, no subprocesses, no real network, no real clock. Every cross-crate dep mocked through its `pub` trait surface. | **Yes.** This is the primary coverage-bearing layer — push as much behavior here as is feasible. |
| **3. Technical unit** | Corner cases and internal invariants that don't fit a Gherkin scenario (parser edge cases, deterministic error paths, internal type invariants). | `#[cfg(test)] mod tests` blocks inline in `src/**`. Has access to `pub(crate)` items by design. | **Forbidden** — same rule as Layer 2. Mocks for cross-crate deps; injected ports for FS / network / process / clock. | **Yes.** |

Two design implications fall out of this split:

- **Each production crate exposes a stable `pub` interface (trait or type) that other crates depend on.** Layer 2 tests of one crate mock the public surface of the crates it talks to. This is dependency-inversion as a first-class architectural rule, not just a testing tactic.
- **Layer 1 is the only place subprocess spawns / real daemons / real network are allowed.** When Layer 2 needs to assert "lns-cli correctly invokes the service to start a run", it does so through a mocked `ServiceClient` trait — not by `tokio::process::Command`-ing the real binary.

The 2 ↔ 3 balance is intentionally weighted toward Layer 2: the more behavior we can pin via Gherkin scenarios against a public API, the better. Layer 3 fills the gaps that aren't expressible as user-facing behaviors (e.g. "if `tokio::fs::rename` fails partway through atomic install, no half-written file remains" — a property, not a behavior).

> **Where each layer lives today:**
>
> - Layer 1 lives in `crates/e2e-tests/` and spawns the real `lns` / `lns-service` binaries against real Unix sockets and tempdir homes. Its profraw is excluded from the coverage gate.
> - Layer 2 lives in `crates/lns-cli/tests/behaviours/` (in-process via `lns_cli::cli::Cli`) and `crates/lns-service/tests/behaviours/` (in-process via `lns_service::ipc::handle_request`). Cross-crate deps are mocked at the public trait boundary (`ServiceClient` in lns-cli; lns-service streaming dispatch has no Handler-style seam yet).
> - Layer 3 exists as `#[cfg(test)] mod tests` blocks. Newer modules (kernel.rs, oci_layer_cache::install) use injected ports (`Fetcher`, `Fs`, `WritableFile`) with in-memory fakes; the still-tempfile-based modules are tracked in IGNORES under "needs ${X} port".
>
> Do not extend the cross-crate subprocess-spawning pattern. New behaviour goes in Layer 2 / Layer 3 with mocked cross-crate deps; only true end-to-end wiring confirmations belong in Layer 1.

### Extracting library seams

Extract a library entry point (a port / trait) when **either** of the following is true: (1) a Layer 2 test needs to assert internal state or mock a dependency the binary can't fake from outside, or (2) a module owns logic whose error paths or platform-specific branches can only be exercised by injecting a fake side-effect (filesystem, process runner, clock, network, FFI). Otherwise, don't extract — the cost of premature abstraction still applies. The litmus test for case (2): "is this an error path that real I/O can't deterministically trigger, or a branch that only fires under another OS / kernel version / network condition?" If yes, a small injected port + in-memory fake is justified.

### Out of scope (any layer)

Scenarios that require booting a real microVM (`lns run <image>`) need Vz/KVM and (for OCI-image variants) network, so the **PR gate** doesn't run them — `cargo test` and the required CI jobs stay virt-free. They are not unrun, though: the `@microvm`-tagged scenarios in `crates/e2e-tests/features/` boot a real guest via `make e2e-microvm` (macOS Vz, or Linux Cloud Hypervisor when `/dev/kvm` + cloud-hypervisor + virtiofsd are present — the target preflights both). CI exercises them on the **non-required** nightly `e2e-microvm` workflow (x86_64 KVM runner); run `make e2e-microvm` locally on a virt-capable host otherwise. Scenarios that still can't run imageless (volume seeding, server images) stay parked under `crates/e2e-tests/specs/microvm/` with no step glue until `lns image import` lands. Corner cases not expressible against a real guest remain covered by Layer 3 unit tests in `src/vm/`.

### Feature file conventions

- One feature per file; one concern per feature.
- Keep `help` and `version` (and any other distinct behaviors) in separate features.
- Scenarios stay focused — each scenario is one user story.
- Layer 2 features live under `crates/<prod>/tests/behaviours/`; Layer 1 features live under `crates/<prod>/e2e/` and are picked up by the `crates/e2e-tests/` harness.
- Adding a new `.feature` to an existing layer doesn't require a new test bin — the layer's cucumber harness recursively globs its features dir. If a step phrasing doesn't exist yet, add it to the layer's `steps/` module alongside the existing ones.

## Commands

Run from the workspace root. For crate-scoped iteration, use cargo directly: `cd crates/foo && cargo test` (or `cargo test -p foo` from anywhere).

```
make dev             cargo build -p lns-cli -p lns-service (debug, skips cross-builds — inner loop)
make build           build + macOS-codesign + copy bin/lns and bin/lns-service (shipping artifacts)
make build-lns       just bin/lns
make build-lns-service  just bin/lns-service
make lint            cargo fmt --all -- --check + cargo clippy --workspace --all-targets -- -D warnings
make test            cargo test --workspace --exclude e2e-tests --all-targets
make complexity      per-crate cargo clippy -- -D clippy::cognitive_complexity (feature-unification)
make fmt             cargo fmt --all
make coverage          full workspace coverage gate (all crates, 100% floor)
make coverage-affected coverage gate narrowed to crates touched since origin/main
make coverage-lcov     re-emit the last coverage run as target/llvm-cov-target/llvm-cov/lcov.info (no rerun)
make e2e             Layer 1 cucumber harness against real lns + lns-service binaries
make parity          re-run the built test binaries under a perturbed environment
make gate-report     per-step gate timings recorded by scripts/gate-timing.sh
make clean           rm -rf bin/ + cargo clean (coverage artifacts live inside target/)

# `make coverage` writes only the lcov the gate reads. Add the text summary
# and the browsable HTML report when you need them:
COVERAGE_HTML=1 make coverage

# CI invokes the same gate targets with strictness toggled on:
CARGO_LOCKED=--locked make lint
CARGO_LOCKED=--locked make test
```

Adding a new crate that needs the per-crate `complexity` gate step: add it to `GATE_CRATES` in the root Makefile. If it also produces a shipping artifact, add a `build-<crate>` recipe and wire it into `build`.

## Toolchain

Rust toolchain is pinned via `rust-toolchain.toml` (currently `1.97.1`). One-time setup:

```
cargo install cargo-llvm-cov   # for `make coverage` / `make coverage-lcov`
```

If you use mise, run `mise trust && mise install` too — `.mise.toml` points mise at `rust-toolchain.toml`, and until it is trusted every mise command errors, then warns `missing: rust@<pin>` until the pinned version is installed.

`cargo-llvm-cov` measures **Layer 2 + Layer 3 only** (see [Test layers](#test-layers)). It wraps `cargo` with LLVM source-based instrumentation. Layer 1 (E2E) tests are deliberately excluded from coverage scoring — their value is wiring confidence, not line attribution. Vendored upstream code (`composefs/vendor/`), the build script, and the thin production-wiring adapters (`kernel/real.rs`, `kernel/traits.rs`, `service/real.rs`) all flow through the same IGNORES table in `scripts/coverage-floor.sh` — one mechanism, one place to look.

The Layer 1 cucumber crate (`crates/e2e-tests`) is excluded from `cargo test --workspace --exclude e2e-tests` in `coverage-data`, so its subprocess spawns do not contribute to any crate's coverage data — `make e2e` runs it separately for wiring confirmation only.

### Per-file coverage gate

The gate decomposes into two layers:

- `make coverage-data` (workspace one-shot) builds the AST-stripped `lcov.info` from `cargo test --workspace --exclude e2e-tests --all-targets`. The single-pass approach is a deliberate simplification — now that Layer 2 is in-process and Layer 1 is excluded entirely, a per-crate `cargo llvm-cov test -p <crate>` rotation would produce equivalent data; the workspace pass keeps the Makefile small.
- `make coverage` runs `coverage-data` and then iterates every `crates/*/` directory, invoking `scripts/coverage-floor.sh <lcov> <crate-prefix>/` once per crate. Each crate reports its own OK/SKIP/FAIL block. The lcov is shared; the per-file gate is per-crate.

For crate-scoped coverage inspection during development, run `scripts/coverage-floor.sh target/llvm-cov-target/llvm-cov/lcov.info crates/<crate>/` directly (assumes `make coverage-data` has already built the lcov).

**Every file in the (post-strip) lcov must be at 100% UNLESS it's listed in IGNORES**. The IGNORES table lives at the top of `scripts/coverage-floor.sh`. Each entry is `<path-suffix>  <reason>`; the reason is mandatory — reviewers should reject ignore entries without one. **The table is the tracker.** There's no parallel Jira/issue list; the entry's reason names the design move that gets the file to 100%, and the entry leaving the table is the completion signal. Reasons fall in three buckets:

1. **Platform-only** — the file only compiles or only meaningfully runs on a target the dev/CI host can't be (Linux microVM, macOS Virtualization.framework, etc.). Permanent unless the host story changes.
2. **Top-level binary main** — the `lns` / `lns-service` bootstraps are intentionally exercised only by Layer 1 (E2E) tests; Layer 2/3 don't drive them. Gate the rest of the crate at 100%; accept the bootstrap as covered by E2E (which is not measured by design).
3. **Pending refactor** — file needs port extraction or a similar design change before it can be deterministically tested. The reason names the port (Fs, Process, Signal, …); drop the entry when the file reaches 100%.

Rules:

- **No per-line "exempt this" escape hatch.** See [Reaching 100%](#reaching-100) for the design moves that make every line testable.
- **Ratchet upward only.** A file leaves the IGNORES list when it reaches 100% and stays out. Re-adding it needs a justification in the PR description.
- **New source files are gated automatically.** When `crates/foo/src/bar.rs` shows up in the lcov, the gate enforces 100% on it. Add to IGNORES with a reason if it's intentionally exempt — but that should be the rare case.
- **New side-effect-isolated modules go in at 100% from day one.** A module that uses injected ports (filesystem, process runner, clock, network) with in-memory fakes has every error path reachable from a unit test, so it never needs IGNORES.

The coverage gate is wired into `pre-push` via the in-tree hook at `scripts/hooks/pre-push`. One-time setup per checkout:

```
make install-hooks
```

That points `core.hooksPath` at the in-tree hooks dir so `make lint`, `make complexity`, and `make coverage-affected` run before every push. The hook refreshes `origin/main` before computing the affected set. Override to full workspace: `LNS_PREPUSH_FULL_COVERAGE=1 git push`. Bypass entirely (rare, intentional): `git push --no-verify`.

### Affected-crates coverage (CI + pre-push)

`scripts/affected-crates.sh <base-ref>` determines which workspace crates need coverage based on the diff between `HEAD` and the merge-base of the given ref. It emits one of three outcomes:

- **`__FULL__`** — run full workspace coverage. Fires when the diff touches anything that could change coverage semantics globally: `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, `Makefile`, `crates/*/Makefile`, `scripts/coverage-floor.sh`, `scripts/affected-crates.sh`, `crates/coverage-strip-ast/*`, `crates/e2e-tests/*`. Also fires as a safe fallback when the base ref is unresolvable or `jq` is missing.
- **`__NONE__`** — skip coverage entirely (exit green). Fires when every changed path is docs, CI config, editor tooling, or JS linting infrastructure (`.md`, `docs/`, `.github/`, `package.json`, `.vscode/`, etc.).
- **Crate list** — newline-separated bare crate names (e.g. `lns-cli`). The script computes the reverse-dependency closure of directly-touched crates using `cargo metadata` path-dep edges, so changing `lns-ipc` also gates `lns-cli` and `lns-service`. Infra crates (`e2e-tests`, `coverage-strip-ast`) are excluded from the list — they trigger `__FULL__` on direct touch instead.

The root `Makefile` exposes `make coverage-affected` (defaults `BASE_REF=origin/main`), which dispatches on the script's output: skips on `__NONE__`, delegates to `make coverage` on `__FULL__`, or passes `COVERAGE_CRATES="<list>"` to `make coverage` to narrow both the test run and the per-file gate to just the affected crates.

**Local pre-push** runs `make coverage-affected` by default. This keeps the feedback loop fast on branches that only touch one or two crates. Two escape hatches:

- `LNS_PREPUSH_FULL_COVERAGE=1 git push` — force full workspace coverage (useful before a release tag or to reproduce a CI `push: main` run locally).
- `git push --no-verify` — skip the hook entirely.

**CI** uses the same script on `pull_request` events so local and CI stay in sync — a green pre-push predicts a green PR check. On `push: main` (post-merge), CI runs full workspace coverage unconditionally to maintain the authoritative baseline.

### Reaching 100%

Coverage is a downstream signal of test quality, not a target. Every test must justify a piece of production code — TDD-style. Each unit test's name and body should answer "what behavior am I pinning, and why does it matter?" If the only answer is "covers line N", either the production code shouldn't exist, or the behavior belongs in a different layer.

With that framing: a line that's "uncoverable" means the surrounding code is under-designed for testability, but the test that ends up covering it still has to pin a real behavior — never just touch the line. Layer 2 (behavioural Gherkin) carries the "why" for user-visible behavior; Layer 3 (technical units) fills the gaps Layer 2 can't isolate (deterministic error paths, parser edge cases, internal invariants, etc.). The common "uncoverable" excuses and the actual fix:

- **Tracing macros (`log::debug!`, `log::warn!`, etc.).** Both arms of the macro's `if subscriber_enabled { dispatch } else { drop }` are reachable. The pinned behavior is "this codepath emits the right structured event with the right fields" — assert on captured events, not just that the line ran. Use `tracing::subscriber::with_default(enabled_layer, || ...)` to install a capturing subscriber for the test; run a sibling with a disabled layer when both arms are real production states (debug-off shipping config and debug-on developer config).
- **Platform-only branches under `#[cfg(target_os = ...)]`.** Factor the platform call behind a port and ship a host fake. The `#[cfg]` block becomes a one-line delegation, and the pinned behavior is "the orchestration around the platform call is correct" — tested via the fake — rather than the syscall itself.
- **Production wiring adapters** (e.g. `pub fn ensure() { Resolver::production().resolve() }`). Construction has no I/O, so `Resolver::production()` gets a pure-assertion test that pins "the wired-up defaults match the manifest" (cache path, CDN base, pinned sha, etc.). The adapter itself gets a `serial_test`-serialized test that pins "the override env var short-circuits before any network" using a benign `LNS_KERNEL_PATH=<tempfile>`.
- **Network I/O.** Inject a `Fetcher` port + `RealFetcher` + fake. The pinned behaviors are the real failure modes — "registry returns 500 → bail with context", "received bytes hash to wrong sha → bail before atomic_write so a tampered artifact never lands" — not "we called fetch once".
- **Filesystem error paths.** Inject an `Fs` port. Tempfile-based tests are tolerated for legacy modules, but new code uses a port so each test pins a real failure mode: "mkdir denied surfaces a useful error", "fsync failure aborts the install and leaves no half-written file", "rename collision is the cached-already case and is benign".
- **Defensive `unreachable!()` / `panic!()`.** If the branch is truly impossible, prove it with the type system and delete the panic. If it's "shouldn't happen, but…", construct the pathological input and pin the assertion message — that's a real safety property, not coverage padding.

**LLVM source-mapping artifacts (doc comments attached to items, trait/impl block headers, blank lines between items, multi-line macro `);` closures, closer-only lines that include `?` such as `)?;` from `fn(\n    arg,\n)?;`) are not a coverage hole.** `make coverage` runs `coverage-strip-ast` (`crates/coverage-strip-ast`), an `syn`-based post-processor that walks the AST of every `SF:` source in the lcov and drops `DA:` entries on lines outside any executable position (function signatures, statement starts, expression spans, function-body braces). What llvm-cov over-includes, the strip step removes — automatically and uniformly, no per-line annotations.

A consequence worth being explicit about: a closer-only line whose only non-whitespace characters are `}`, `)`, `]`, `;`, `,`, `?` (or whitespace) is treated as non-executable, including the `?` case. This means the `?` branch BB that LLVM source-maps to a multi-line call's closing `)?;` line is **not** measured at that line. The error-path coverage signal for these sites lives in dedicated tests of the surrounding `Result`-returning call (e.g. "fsync failure aborts the install and leaves no half-written file"), not in whether the specific `?` short-circuited. If you're writing new code that depends on a specific `?` branch being measured, reshape the call so `?` lands on the same line as the expression (single-line form, or `let x = call(...).context("…")?;`) — that line is executable and the DA hit counts.

The fallback markers (`// coverage:ignore-line`, `coverage:ignore-start`/`-end`, and the `LCOV_EXCL_*` aliases) are still honored by `coverage-strip-ast` for backward compatibility — they are **not** a sanctioned way to land new code. A PR that adds a new marker needs a different design instead. Existing markers in the tree are tech debt to remove as the surrounding modules are refactored to the port pattern.
