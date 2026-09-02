---
name: problem-first-impl
description: >
  Continuation of `/problem-first`. Confirms scope against product docs and
  existing specs, writes the agreed Gherkin scenarios to a `.feature` file
  in the target crate's test-layer directory, and enters technical planning
  mode. Use when the user explicitly runs `/problem-first-impl`, or when
  they ask to proceed to implementation after a `/problem-first` session.
  Do NOT auto-invoke.
---

# Spec — Problem-First → Implementation

Take the agreed problem + Gherkin scenarios from `/problem-first` and move to technical planning. This skill does two things: (1) a final scope confirmation against the product docs and existing specs, and (2) hand-off — writing the `.feature` file and entering plan mode.

This skill is the "code path" continuation. If the goal is to file an issue for someone else to pick up, use `/problem-first-issue` instead.

## Inputs

You need the full output of a `/problem-first` session, or a GitHub issue carrying the same content:

1. A **problem statement** (1-2 sentences).
2. An agreed set of **Gherkin scenarios**.
3. A **target crate** in this monorepo.
4. An agreed **approach** — one-paragraph solution direction from `/problem-first` Phase 3.
5. **Alternatives considered** and their rejection reasons (1-3).
6. Optionally: **assumptions / constraints**, **non-goals**, **open questions**.

### Invocation modes

**Mode A — Continuation in the same session as `/problem-first`.**
If `/problem-first` just ran in this conversation, read the full output (problem, scenarios, target crate, approach, alternatives, assumptions) directly from conversation context. Doc-level alignment was confirmed at intake — do not re-run that check. Briefly confirm: *"Using the problem, N scenarios, target crate `<crate>`, and chosen approach from the `/problem-first` session above (doc-alignment confirmed) — proceeding to code-level check."*

**Mode B — Standalone (picking up a GitHub issue, or resuming cold).**
If there is no prior `/problem-first` output in the session, the user is likely picking up work that was planned earlier. Either:

- Ask for the source: *"Do you have a `/problem-first-issue` GitHub issue for this? Paste the number or URL and I'll read it (`gh issue view <n>`), or paste the problem + scenarios + approach directly."*
- Or elicit all six input items above.

In Mode B, product docs may have changed since the issue was filed. Re-run the doc-level alignment check from `/problem-first` Phase 4 before the code-level check — cheap insurance against silently implementing against stale direction.

Do NOT fabricate content. If the approach is missing, do not proceed — send the user to `/problem-first` Phase 3 to decide direction before writing a `.feature` file. The approach is what makes the code-level check and the technical-planning phases meaningful; without it, this skill degrades into guessing.

If the target crate is unclear, stop and determine it before continuing — the `.feature` file needs a home.

## Process

`/problem-first` covered Phases 1-4 (problem, behavior, direction, doc-level alignment). This skill continues from there with code-level checks and implementation.

### Phase 4: Code-Level Check

Doc-level alignment was confirmed at intake. This phase is the complementary check against the actual source tree — the things that can only be verified with the checkout in front of you.

1. **Existing specs.** Read the target crate's feature files — `tests/behaviours/*.feature` (Layer 2) and, if relevant, `e2e/*.feature` (Layer 1). Do any overlap with the agreed scenarios? Overlapping scenarios must be reconciled (merge, rename, or replace), not silently duplicated.
2. **Approach vs current source.** Open the crate. Does the agreed approach still fit the code as it exists today? Has the crate drifted since `/problem-first` ran? If it has, name the drift and decide: does the approach still hold, or do we go back?
3. **Re-verify assumptions.** Walk through each load-bearing assumption from `/problem-first` Phase 3. Can you confirm it from the code? An assumption that looked safe at intake may be wrong in practice — if so, the approach may need revisiting. Send the user back to `/problem-first` rather than papering over it.

Then present a summary:

- Problem statement (1-2 sentences)
- Target crate(s)
- Behavior scope (list the scenario titles, plus any reconciliation with existing specs)
- Agreed approach (one paragraph)
- Assumptions that still hold (and any that shifted, with what that means)
- Explicit non-goals
- Any new conflicts surfaced by looking at the code
- Any docs that need updating if this work lands

Ask: **"Are we aligned? Should I proceed to writing the `.feature` file and technical planning?"**

Do not proceed without explicit confirmation.

### Phase 5: Hand-off to Technical Planning

Only after the user confirms alignment:

1. Write the agreed Gherkin scenarios to the appropriate `.feature` file, following the repo's **Test layers** convention (see the repo-root `CLAUDE.md`). Default to **Layer 2** behavioural specs at `crates/<crate>/tests/behaviours/` — that's the primary coverage-bearing layer for user-visible behavior. Only put a scenario in **Layer 1** (`crates/<crate>/e2e/`) if it genuinely requires real binaries / a real microVM / real I/O. Match the file naming and style of existing `.feature` files in that directory; if a suitable existing file exists, append to it, otherwise create a new one (and add a step definition stub under `tests/behaviours/steps/` if the phrasing is new).
2. Enter plan mode for technical implementation planning, starting from the agreed approach — not from a blank page. The plan refines the approach into concrete types, files, and steps. It should not reopen the "what direction" question; if it needs to, that's a scope drift and we go back to `/problem-first`.

The Gherkin file is the contract. Implementation must satisfy it, nothing more, nothing less.

If the change has non-obvious unit-test scope (e.g. internal helpers, error-mapping logic not visible at the Gherkin level — Layer 3 territory), note it in the plan — but Gherkin scenarios remain the acceptance bar. Each plan step should fold in its own failing-test-first work rather than deferring all tests to a tail-end step.

## Rules

- **Phases 1-3 are `/problem-first`'s job.** If inputs are missing, send the user back there rather than re-deriving them here.
- **Preserve the agreed Gherkin exactly.** Do not rephrase scenarios when writing the file.
- **Use canonical terminology** from `docs/` and the Product Vision in the repo-root `CLAUDE.md`.
- **If scope or direction drifts during Phase 4 or Phase 5**, go back to `/problem-first` rather than papering over it here.
