# Mutation Test

Verify test quality by mutating source code and checking if tests catch it. Uses [cargo-mutants](https://github.com/sourcefrog/cargo-mutants) for Rust mutation testing.

**Scope:** any crate under `crates/*/` that exposes a `mutation-test` Makefile target. New crates are picked up automatically.

The full tree runs nightly on `main` via the `Mutation Test (nightly)` workflow and uploads `crates/*/mutants.out/` as the `mutants-out` artifact. This skill covers both ad-hoc local runs and triage of the nightly artifact.

## When to use

- After implementing a feature — verify your tests actually catch bugs
- After fixing PR review comments — confirm the fix is tested
- When reviewing test quality for a crate
- When triaging the nightly `mutants-out` artifact into a follow-up PR

## Usage

```
/mutation-test                          # whole tree (every crates/*/ with a mutation-test target)
/mutation-test <crate>                  # single crate (e.g. lns-cli)
/mutation-test <crate> <path>           # specific file or module within a crate (e.g. lns-cli src/lib.rs)
```

## Workflow

### Step 1: Determine target

| Argument | Target | Command |
|----------|--------|---------|
| (none) | Whole tree | `make mutation-test` from repo root |
| `<crate>` | One crate | `make -C crates/<crate> mutation-test` |
| `<crate> <path>` | One file/module | `cd crates/<crate> && cargo mutants --no-shuffle --file <path>` |

The single-file form bypasses the crate's Makefile, so any env-var setup the Makefile owns (e.g. `LNS_INIT_BIN=skip` in `crates/lns-cli/Makefile`) won't apply. If the file-scoped run misbehaves, fall back to the crate-level `make` invocation.

### Step 2: Run cargo-mutants

`cargo-mutants` writes results to `mutants.out/` under whatever directory it runs in (gitignored). For crate-level runs that's `crates/<crate>/mutants.out/`. For the root `make mutation-test`, every participating crate gets its own `mutants.out/`. The summary at the end of the run lists totals; per-mutant detail lives in `mutants.out/outcomes.json` and the `caught.txt` / `missed.txt` / `unviable.txt` / `timeout.txt` files.

### Step 3: Parse results

Read `mutants.out/outcomes.json` for the structured view. Each entry has a `scenario` (the mutation), the affected file/line, and an `outcome`:

- **caught** — tests killed the mutation (good)
- **missed** — tests passed despite the mutation (weak test)
- **unviable** — mutation didn't compile (ignore)
- **timeout** — mutation hung; raise the timeout or investigate the test

The plain-text companions are quicker to scan:

```bash
cat mutants.out/missed.txt   # survivors that need fixing
cat mutants.out/caught.txt   # killed mutants — sanity check coverage
```

When triaging the nightly artifact, do this for every `crates/*/mutants.out/` directory in the downloaded archive.

### Step 4: Fix every survivor

For each entry in `missed.txt`:

1. **Read the source** at the reported file:line
2. **Understand the mutation** — `cargo-mutants` lists the operator (e.g. `replace == with !=`, `replace + with -`, `delete !`, `replace function body with Default::default()`)
3. **Write the fix** — every survivor gets one of:
   - **New test** — if no test exercises this code path, write one
   - **Tighter assertion** — if a test runs the code but doesn't check precisely, fix the assertion
   - **Refactor for testability** — if the code can't be tested as-is, refactor it (extract dependencies, add parameters, split functions)

**There are no acceptable excuses for leaving a mutant alive.** "Equivalent mutant" means the code has redundant logic that should be simplified. "Untestable" means the design needs to change. The only exception is a literal no-op mutation (e.g. an unobservable timeout in fire-and-forget shutdown) — flag these explicitly in the report and justify why.

### Step 5: Re-run and confirm

After writing fixes, re-run the same command (root, crate, or file). Repeat until:

- **0 missed** mutants (only justified equivalents survive)
- **>95% efficacy** — `caught / (caught + missed)`

### Step 6: Analyze density

For each file, count total mutants. High density signals:

| Mutants | Signal |
|---------|--------|
| 1–10 | Normal — focused module |
| 11–20 | Watch — getting complex |
| 21+ | Split candidate — too much logic in one file |

Cross-reference with function-level analysis: if one function has 5+ mutation sites, it likely mixes concerns.

### Step 7: Report

Present findings as a table. When reporting on multiple crates (whole-tree or nightly triage), include a per-crate efficacy row before the file-level density:

```
## Mutation Test Results

**Overall: 97.8% efficacy | 44 caught, 1 missed, 0 timeouts**

### Per-crate efficacy

| Crate | Caught | Missed | Score |
|-------|--------|--------|-------|
| lns-cli | 44 | 1 | 97.8% |

### Survivors

| Crate | File | Line | Mutation | Justification |
|-------|------|------|----------|---------------|
| lns-cli | src/lib.rs | 42 | replace `==` with `!=` | (fix or justify) |

### Density

| Crate | File | Mutants | Caught | Score | Note |
|-------|------|---------|--------|-------|------|
| lns-cli | src/lib.rs | 18 | 18 | 100% | |
```

## Thresholds

| Metric | Target |
|--------|--------|
| Test efficacy | >95% |
| Missed mutants | 0 (or justified) |
| Per-file score | >90% |

## Prerequisites

```bash
cargo install cargo-mutants
```
