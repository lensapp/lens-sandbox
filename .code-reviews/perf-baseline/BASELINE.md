# Verification gate — performance baseline

Cold-cache wall-clock timings for the full pre-push gate (`make verify` + `make coverage`). Captured so we can A/B against the planned cleanup of redundant production builds.

## Environment

- Host: Darwin 24.6.0 arm64, 10 cores, 32 GiB RAM
- Toolchain: rustc 1.95.0 / cargo 1.95.0 (per `rust-toolchain.toml`)
- Commit: `34284582` on `main`
- Workspace state at start: **cold** (`target/` absent)
- `~/.cargo/registry` and `~/.cargo/git` were warm (typical dev machine — no proxy for cold registry fetch)

Per-step logs live alongside this file (`01-build.log` … `05-coverage.log`).

## Per-step wall time

| Step                         | Wall    | Notes                                                                                                                       |
| ---------------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------- |
| `make build` (release)       | 115.35s | Full release compile (cargo: 1m 28s) + per-crate iteration + macOS codesigning + copy to `bin/`. Artifacts not consumed downstream. |
| `make lint`                  | 41.49s  | `cargo fmt --check` + `cargo clippy` per crate. Clippy did a fresh `dev` profile compile (27.93s on the last crate) — release artifacts from `build` don't help. |
| `make test`                  | 73.72s  | Includes the redundant `cargo build -p lns-service` from `crates/lns-cli/Makefile:41` — that line alone shows as a 29.16s `dev` profile build in the log. |
| `make complexity`            | 13.43s  | Mostly warm at this point (debug check artifacts cached from lint/test).                                                    |
| **`make verify` subtotal**   | **244.0s** | Sum of the four above. This is what the pre-push hook runs first.                                                          |
| `make coverage`              | 54.65s  | Includes `cargo build --release -p coverage-strip-ast` (13.96s), `cargo llvm-cov clean --workspace`, the redundant `cargo build -p lns-service` (7.41s instrumented dev), the instrumented test pass (24.17s), llvm-cov reports, AST strip, and per-crate floor iteration. |
| **Pre-push gate total**      | **~298.6s** (~5m) | `make verify` then `make coverage`. This is the wall time enforced by `scripts/hooks/pre-push`.                          |

## Redundancies confirmed by the logs

1. **`make build` produces release artifacts no later step consumes.** Every consumer (`lint`, `test`, `complexity`, `coverage`) re-compiles in `dev` / `test` / instrumented mode. Net cost: ~115s of cold wall, plus macOS codesigning.
2. **`crates/lns-cli/Makefile:41` `cargo build -p lns-service`** runs before `cargo test --all-targets`. Layer 2 lns-cli tests are in-process (no subprocess spawn — confirmed by `grep -r "Command::new\|process::Command" crates/lns-cli/tests/` → no hits). The log attributes ~29.16s of cold wall to this dev build.
3. **`Makefile:125` `cargo build -p lns-service`** runs before `cargo test --workspace --exclude e2e-tests`. Same reasoning — `e2e-tests` is excluded, so nothing in the test set spawns the binary. Log attributes 7.41s to this instrumented dev build.
4. **`crates/lns-ipc/Makefile:24-25` `cargo build --release`** on a pure-library crate. No shipping artifact, no consumer — pure waste.

## Reduction target for the cleanup

Conservative estimate (cold cache, this hardware):

| Change                                                       | Expected saving |
| ------------------------------------------------------------ | --------------- |
| Drop `make build` from the pre-push gate (or replace with `cargo check --release`) | ~80–110s        |
| Drop `cargo build -p lns-service` from `crates/lns-cli/Makefile` `test:`        | ~29s            |
| Drop `cargo build -p lns-service` from top-level `coverage-data:`               | ~7s             |
| Make `crates/lns-ipc` `build:` a no-op or `cargo check`                          | ~3–5s           |
| **Target gate total**                                                            | **~150–180s** (~2.5–3m) |

Warm-cache numbers will compress everything; the ratio of waste should remain similar.

## Re-measure procedure (post-cleanup)

```sh
cargo clean
{ /usr/bin/time -p make build      > 01-build.log      2>&1; }
{ /usr/bin/time -p make lint       > 02-lint.log       2>&1; }
{ /usr/bin/time -p make test       > 03-test.log       2>&1; }
{ /usr/bin/time -p make complexity > 04-complexity.log 2>&1; }
{ /usr/bin/time -p make coverage   > 05-coverage.log   2>&1; }
```

Run each step serially (no parallel `make` jobs) so the times aren't skewed by `target/` lock contention.
