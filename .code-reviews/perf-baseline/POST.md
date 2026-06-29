# Verification gate — post-cleanup measurement

Same host, same toolchain, same cold-cache procedure as `BASELINE.md`. Per-step logs:
- v1 (introduce `check`, drop redundant `cargo build -p lns-service`): `post/01-check.log` … `post/05-coverage.log`
- v2 (drop `--release` from `check` to share dev-profile artifacts with downstream steps): `post2/01-check.log` … `post2/05-coverage.log`
- v3 (one-truth: workspace lint/test in top-level Makefile, CI uses `CARGO_LOCKED=--locked make lint/test`, drop `check` entirely, isolate coverage in target-cov/): `post3/01-lint.log` … `post3/04-coverage.log`

## Changes between BASELINE and POST

- `crates/lns-cli/Makefile`, `crates/lns-service/Makefile`, `crates/lns-ipc/Makefile`: introduced a `check:` target (`cargo check --release --all-targets`, with `LNS_INIT_BIN`/`LNS_SESSION_BROKER_BIN=skip` where build.rs needs it) and pointed each crate's `verify:` at `check lint test complexity` instead of `build lint test complexity`. `build:` still exists — it's now the shipping-artifact target, run on demand, not part of the gate.
- `crates/lns-cli/Makefile`: dropped the redundant `cargo build -p lns-service` from `test:`. Layer 2 lns-cli tests are in-process; the binary is never spawned.
- `Makefile` (root): added top-level `check:`, added `check` to `REQUIRED_TARGETS`, dropped the redundant `cargo build -p lns-service` from `coverage-data:`, and refreshed the stale comment block above `coverage-data` (it was still claiming the workspace pass had to be one-shot because acceptance tests spawn subprocesses — that hasn't been true since Layer 1 was excluded and Layer 2 went in-process).
- `CLAUDE.md`: rewrote the Verification gate step 1 to describe `make check`, updated the per-crate commands table to include `check` (and to note that `build` is no longer in the gate), and added `check` to the `check-contract` required-target list.

## Side-by-side

| Step                         | Baseline | v1       | v2       | v3       | Δ vs baseline       |
| ---------------------------- | -------: | -------: | -------: | -------: | ------------------: |
| Compile (`build` / `check` / `lint+check fused`) | 115.35s | 46.26s | 39.32s | n/a — gate uses lint directly | n/a |
| `make lint`                  | 41.49s   | 42.65s   | 17.62s   | 32.51s   | −8.98s (−22%)       |
| `make test`                  | 73.72s   | 42.96s   | 41.59s   | 43.01s   | **−30.71s** (−42%)  |
| `make complexity`            | 13.43s   | 13.05s   | 12.94s   | 23.14s   | +9.71s              |
| **`make verify` subtotal**   | **244.0s** | **144.92s** | **111.47s** | **98.66s** | **−145.34s (−60%)** |
| `make coverage`              | 54.65s   | 40.93s   | 41.26s   | 63.61s   | +8.96s              |
| **Pre-push gate total**      | **~298.6s (~5m)** | **~185.85s (~3m)** | **~152.73s (~2.5m)** | **~162.27s (~2.7m)** | **−136.33s (−46%)** |

### v3 deltas explained

- **Compile step removed.** `make check` was dropped — clippy in `make lint` is its strict superset. The gate's compile signal is now `make lint` itself.
- **`make lint` +15s vs v2** because v2 had `make check` warming the dev-profile dep cache; v3 makes lint do that work in one step (no separate check pass). Wall-time wise, v3's "lint = 32s" replaces v2's "check (39s) + lint (17s) = 56s". So fewer total seconds, fewer total invocations.
- **`make complexity` +10s** because v3 reverted complexity to per-crate iteration (it has to: workspace clippy with `-D cognitive_complexity` catches functions per-crate clippy doesn't — known feature-unification difference; existing CI design already split this out). Per-crate clippy can't share workspace lint's cache slot, so it does more work cold.
- **`make coverage` +22s vs v2** because coverage now uses `target-cov/` (isolated from `target/`) — required for one-truth with CI, and to avoid cargo's fingerprint reusing non-instrumented test binaries from the `make test` step. Local now mirrors CI behavior exactly.
- **`make verify` subtotal best yet.** 98.66s cold workspace > sequential per-crate iteration ever achieved.

## Attribution

- **Compile (−76s, −66%).** `cargo check` skips codegen + linking + macOS codesigning for the two binaries and skips codegen for the lns-ipc library. The v1→v2 delta (−7s extra) comes from dropping `--release` — same typechecked paths, smaller artifact set.
- **Lint (−24s, −58%).** All in v2. v1's `check --release` warmed a release-profile dep cache that clippy (dev profile) couldn't reuse, so lint did a fresh 30s compile. v2's `check` (dev profile) warms exactly the cache clippy wants → lint compresses to ~15s.
- **Test (−32s, −44%).** Almost entirely from dropping the redundant `cargo build -p lns-service` in `crates/lns-cli/Makefile`'s `test:` (baseline log shows `Finished dev profile in 29.16s` attributable to that line). Layer 2 lns-cli tests are in-process and never spawn the binary — confirmed by `grep -r "Command::new\|process::Command" crates/lns-cli/tests/` → no hits.
- **Coverage (−14s, −24%).** Dropped the same redundant `cargo build -p lns-service` from `coverage-data:` (baseline log: instrumented dev build at 7.41s). The remaining delta reflects fewer artifacts surviving the `cargo llvm-cov clean` round-trip.

## Gate health

All five steps exited 0. Coverage gate: 0 failures across `bump-kernel`, `coverage-strip-ast`, `lns-cli`, `lns-init`, `lns-ipc`, `lns-service`, `lns-session-broker`, `lns-session`, `lns-vsock-bridge` (per `post/05-coverage.log` final summary).

## What this means for `make build`

`make build` still exists and still produces the shipping artifacts (`bin/lns`, `bin/lns-service`, codesigned on macOS). It just isn't on the hot path of every push anymore. Devs run it on demand when they want a built binary; CI runs it as part of the release pipeline. The gate's compile signal — "does this code compile?" — is preserved by `cargo clippy` (which compiles + lints in one pass).

## One truth (v3)

After v3, the four gate targets — `lint`, `test`, `complexity`, `coverage` — are defined ONCE in the top-level Makefile. Local devs invoke them directly (fast, relaxed). CI invokes the same targets with `CARGO_LOCKED=--locked` prefix (strict, parallel jobs). Per-crate Makefiles still expose `lint` / `test` / `complexity` as crate-local conveniences for `cd crates/foo && make test` iteration, but the workspace gate doesn't go through them — eliminating the drift surface where CI could call `cargo` directly while local called `make` and the two diverged on flags or scope.

CI wall-clock estimate per job (cached):
- Lint job (`make lint` + `make complexity`): ~50–60s
- Test job (`make test` + `make e2e`): ~50–60s
- Coverage job (`make coverage`): ~60–80s
- Total wall = max of the three (parallel) ≈ ~60–80s, same as before.
