# Contributing to Lens Sandbox

Thanks for your interest in Lens Sandbox.

This repository is the Lens Sandbox monorepo — the `lns` developer CLI, the tray-resident `lns-service`, the guest-side microVM components, and their shared libraries. [`AGENTS.md`](AGENTS.md) at the repo root is the authoritative guide to the architecture, the test pyramid, and the project conventions; this file is the short on-ramp.

## Development Setup

The Rust toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml); `rustup` installs the pinned version automatically on first build. One cargo tool backs the verification gate:

```bash
cargo install cargo-llvm-cov   # make coverage
```

Wire the repository hooks into your checkout (one-time):

```bash
make install-hooks   # points core.hooksPath at scripts/hooks
```

Fast inner loop:

```bash
make dev    # debug build of lns + lns-service, skips cross-builds
make test   # cargo test --workspace --exclude e2e-tests (uninstrumented)
```

## The Verification Gate

Run the full pre-push gate from the workspace root before opening a pull request:

```bash
make lint && make complexity && make coverage
```

- `make lint` — `cargo fmt --check` plus `cargo clippy --workspace --all-targets -- -D warnings`.
- `make complexity` — per-crate `cargo clippy -- -D clippy::cognitive_complexity`.
- `make coverage` — runs the test suite instrumented and enforces a 100% per-file line-coverage floor (exemptions live in `scripts/coverage-floor.sh`).

The `make install-hooks` pre-push hook runs this gate (narrowed to affected crates) on every push. CI runs the same targets plus `make test` and `make e2e`.

## Test Layers

New behavior goes in one of three layers (see [`AGENTS.md`](AGENTS.md) for the full pyramid):

- **Behavioural unit** (`crates/<crate>/tests/behaviours/*.feature`) — a crate's user-visible behavior, cross-crate dependencies mocked. This is the primary coverage-bearing layer.
- **Technical unit** (`#[cfg(test)] mod tests` in `src/**`) — corner cases and invariants that don't fit a Gherkin scenario.
- **E2E** (`crates/e2e-tests/`) — real binaries and real I/O, for wiring confirmation only. Not measured for coverage.

## Pull Request Guidelines

- Keep changes small and focused.
- Add or update tests in the right layer for every behavior change.
- A comment is a code smell — prefer a clearer name, a smaller function, or a type over a comment. See the comment policy in [`AGENTS.md`](AGENTS.md).
- No `unwrap()` outside tests, no unjustified `unsafe`, no suppressed lints, no papered-over errors.
- Update documentation when you change public behavior, configuration, or the policy file format.

## Commit Style

Use Conventional Commits. Do not add a `Co-Authored-By` trailer or a generated-with footer.

- `feat: add lns sandbox validate`
- `fix: deny request when no policy rule matches`
- `docs: explain the approval card flow`
- `test: cover the audit-chain verifier`
- `ci: add the kernel-pin drift check`

## Security-Sensitive Changes

Changes touching the microVM boundary, policy enforcement, the audit chain, or the host IPC surface should include a short explanation of the security impact in the pull request description. For vulnerability reports, follow [SECURITY.md](SECURITY.md).
