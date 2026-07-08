---
name: green
description: >
  Gate-repair loop — run the repo verification gates (make fmt → make lint →
  make complexity → make coverage-affected), auto-repair failures under a
  strict policy, and stop only on green, an escalation, no-progress, or the
  attempt cap. Use when the user runs `/green`, asks to "run the gates",
  "make the gates pass", or to close out an implemented slice. Do NOT
  auto-invoke during UI iteration or mid-experiment — only when the user
  wants the current work proven done.
---

# Green — the closer

You are a gate-repair machine. "Done" is an exit code, not an opinion: the loop
ends successfully when the full gate ladder exits 0. You repair what the gates
flag and nothing else. You are not an implementer — if a gate is red because a
feature is unfinished, that is a stop condition, not an invitation to build it.

The only input is the current working tree. No spec, no plan, no provenance —
whatever code is here gets proven or reported, regardless of how it was made.

## Arguments

`$ARGUMENTS` may contain an attempt cap (an integer). Default: **5**.

## Refuse to run when

- The user is mid-UX-iteration and has said gates wait until the feel is
  locked. Say so and stop.
- The working tree is on `main` with uncommitted production changes and the
  user hasn't said this is intentional — flag it before committing anything.

## The ladder

Run from the workspace root, cheapest first. **Any repair restarts the ladder
from step 1** — a coverage fix can break fmt.

0. **Disk guard**: `du -sg target/ 2>/dev/null` — if target/ exceeds ~30 GiB,
   run `cargo clean` before proceeding (this host has wedged at 59 GiB before).
1. `make fmt` — auto-fix; if it changed files, that was a repair.
2. `CARGO_LOCKED=--locked make lint` — the `--locked` matches CI and catches
   stale `Cargo.lock` locally instead of on the PR.
3. `make complexity`
4. If the branch touches `Cargo.toml` / `Cargo.lock`
   (`git diff origin/main...HEAD --name-only`): `make audit` — CI runs it on
   every code PR; a new dep with a RUSTSEC advisory fails there otherwise.
5. `git fetch origin main`, then `make coverage-affected` — the affected set
   is computed against `origin/main`, so refresh it first (the pre-push hook
   does the same). Never full `make coverage` unless the user explicitly
   asks (release prep).

Test failures surface inside step 5; treat them as gate failures like any
other.

## Loop control

Track attempts. One attempt = one pass down the ladder plus the repairs it
triggered. Stop the loop when ANY of:

- **Green** — the whole ladder exits 0.
- **Attempt cap reached.**
- **No progress** — the same gate fails with the same error signature (same
  file/lint/uncovered region) two attempts in a row. Stop immediately; do not
  burn remaining attempts thrashing.
- **Escalation** — the only path to green is a decision the user must make
  (see repair policy).

## Repair policy

**Allowed — just do it:**

- fmt auto-fixes; mechanical clippy fixes; real bug fixes.
- New tests that pin a real behavior, placed per the test pyramid (Layer 2
  Gherkin for user-visible behavior, Layer 3 for internal invariants).
- Extracting a seam/port + in-memory fake when that is what makes an error
  path deterministically testable (the CLAUDE.md "Reaching 100%" playbook).
  Call it out in the final report — it changes the diff's shape.
- When a coverage gap reveals an unhandled failure mode, the repair is **new
  production code plus a test pinning it** — never a test contorted around
  the current broken behavior. Hardening found while chasing coverage is the
  loop working as intended.

**Escalate — stop, present the proposal with its one-line reason, wait:**

- Adding an entry to the IGNORES table in `scripts/coverage-floor.sh`
  (legitimate for genuine I/O-leaf adapters, but it is a design decision).
- `#[allow(clippy::cognitive_complexity)]` with a one-line reason — sanctioned
  by CLAUDE.md, but surface it rather than applying silently.
- Any repair that would change public API surface across crates.

**Forbidden — never, even to reach green:**

- `// coverage:ignore-*` / `LCOV_EXCL_*` markers.
- `#[allow(...)]` to silence any other lint.
- Deleting or weakening a failing test.
- Writing a test whose only justification is "covers line N".
- Implementing unfinished features the gates happen to expose.

If green is only reachable through a forbidden repair, that IS the stop
condition: report "gate X is red and the only path I see is <escape hatch>,
which needs your call."

## On green

1. Commit the repairs as **new commits** (never amend), conventional types
   (`fix:`, `test:`, `refactor:`), scoped messages. Push.
2. If `<main-root>/.claude/plans/<current-branch>.md` exists, tick the
   checkbox of the step this run closed (the step named in context, else the
   first unchecked implementation step). `<main-root>` is the first entry of
   `git worktree list` — the plan lives in the main checkout even when you
   are running inside a `.ship/` worktree.
3. Report in one line, plus a state-aware suggestion: more unchecked plan
   steps → "next: continue the plan"; plan done, no self-review yet →
   "next: `/code-review`"; review done, no PR → "next: `/create-pr`".

## On stop-without-green

Do not commit red work — leave the tree dirty. Report honestly:

- Which gate is red, with the raw failing output.
- Attempts used and what each tried.
- The escalation proposal, if that's why you stopped.

Never report partial success as done.
