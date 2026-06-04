# bump-kernel

Internal operator CLI: open a PR to bump the lns guest kernel pin.

Edits [`crates/lns-service/kernels.toml`](../lns-service/kernels.toml)'s
`[current]` table to record a new Kata Containers release, clears
the per-arch SHA pins (CI back-fills them), pushes a branch, and
opens a PR via `gh`. Not shipped in the lns binary; only used
by maintainers.

## Quick start

```sh
cargo run -p bump-kernel -- --kata-version 3.31.0
```

The CLI walks through:

1. Validates Kata `3.31.0` exists upstream.
2. Edits `crates/lns-service/kernels.toml` (provenance + cleared SHAs).
3. Asks for confirmation, then commits on a new branch and opens a PR.

CI's `publish-kernel` workflow takes over from there.

## Flags

| Flag | Default | When to use |
|---|---|---|
| `--kata-version <tag>` | required | The Kata release tag (e.g. `3.31.0`; no `v` prefix). Validated against `github.com/kata-containers/kata-containers/releases/tag/<tag>`. |
| `--variant <variant>` | `mainline` | One of `mainline`, `confidential`, `tdx`, `sev`, `snp`, `dragonball`. Mainline is the default non-TEE, non-experimental kernel. |
| `--commit-type {feat,fix,chore}` | `feat` | Conventional-commit type used in the PR subject. `feat` → minor release-please bump; `fix` → patch (for security-driven bumps); `chore` → no immediate release. |
| `--dry-run` | off | Show the planned change without touching git or opening a PR. Restores `kernels.toml` to HEAD on exit. |
| `-y, --yes` | off | Skip the `Continue? [y/N]` prompt. |

## Example: confidential-computing variant

```sh
cargo run -p bump-kernel -- \
  --kata-version 3.31.0 \
  --variant confidential \
  --commit-type fix \
  --yes
```

Opens a PR with subject `fix(lns): bump guest kernel to Kata 3.31.0`,
points at Kata's confidential vmlinuz variant, no interactive prompt.

## Requirements

- `git` (any recent version)
- `curl` (any recent version)
- [`gh` CLI](https://cli.github.com/) installed and authenticated
  (`gh auth login`)
- Clean git working tree (commit or stash uncommitted edits first)
- Cargo + Rust toolchain pinned by the repo's `rust-toolchain.toml`

## What this CLI is NOT

It does not:

- download Kata's tarball (CI does that on the PR — 670 MB is too
  heavy for an operator's machine)
- compute SHA256s (CI does that)
- upload anything to the CDN origin (CI does that, only on merge to main)
- modify `kernel.rs` or anything outside `kernels.toml`

Its job is to **open a well-shaped PR**. Everything else is automated.

## Full operator runbook

[`runbooks/kernel-bump.md`](../../runbooks/kernel-bump.md) — when to
bump, the full PR-to-release flow, variant choice, rollback,
emergency overwrite procedure, common errors. Read it the first time
you do a kernel bump.

## Where the pieces live

| Concern | Location |
|---|---|
| Source of truth (kernel pin) | `crates/lns-service/kernels.toml` |
| Compile-time derivation | `crates/lns-cli/build.rs` |
| Runtime download + verify | `crates/lns-cli/src/kernel.rs` |
| PR-time compute + back-fill, merge-time publish | `.github/workflows/publish-kernel.yml` |
| Drift check (manifest ↔ CDN) | `scripts/check-kernel-pins.sh` |
| CI helpers (publish-kernel) | this crate's `show` + `back-fill` subcommands |
| Operator CLI (this crate) | `crates/bump-kernel/` |
| Full operator runbook | `runbooks/kernel-bump.md` |
