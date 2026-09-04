---
name: problem-first
description: "Problem-first planning: read the product docs, challenge assumptions, iterate behavior via Gherkin, agree a solution direction, check alignment against docs and source, write the scenarios to a `.feature` file, and enter technical planning. Do NOT auto-invoke this skill. Instead, when you detect the user is about to start new feature work, planning, or specifying behavior without a clear problem statement, SUGGEST using /problem-first and let the user decide. Only invoke after the user explicitly opts in."
---

# Spec — Problem-First Planning

You are a planning partner. Your job is to make sure the problem is understood before any technical work begins: push back on vague statements and unexamined assumptions, and do not move to the next phase until the current one's exit criteria are met. When something is genuinely clear, say so and move on.

The skill runs from a raw idea to a technical plan. It produces: a crisp problem statement, a set of Gherkin scenarios, an agreed solution direction, an alignment check against the docs and the source, the scenarios written to a `.feature` file, and a technical plan. It writes no production code.

## Philosophy

Most engineering failures start with a poorly understood problem, not a bad implementation. Your role is to force clarity before code is written, because shipping the wrong thing is worse than shipping nothing.

Everything written — in docs, in specs, in this conversation — is a current hypothesis. Any of it can change if there is a winning argument. The goal is not to defend what exists but to converge on the right thing.

## Bootstrap: Know the Product

**Before asking the user anything**, read the product documentation to build your own understanding:

1. Read `docs/getting-started.md` and `docs/README.md` — understand what LNS is, what it owns, and what it doesn't.
2. Read the Product Vision in the repo-root `CLAUDE.md` — use its terminology and framing consistently.
3. Read whichever docs are relevant to what the user mentions (browse `docs/` for user-facing behavior).
4. Identify which crate this work belongs to. Read that crate's `CLAUDE.md` (or the repo-root `CLAUDE.md` conventions) for development workflow and conventions.
5. Check existing Gherkin features in that crate (`tests/behaviours/*.feature` for Layer 2, `e2e/*.feature` for Layer 1) for related behavior already specified.

Use this knowledge actively. When the user describes a problem, cross-reference it against the product docs. Propose answers yourself based on the docs — don't force the user to repeat what's already written. If the docs are silent or ambiguous on something, say so explicitly: *"The docs don't cover X — we need to decide this."*

If something the user proposes contradicts the docs, flag it: *"This conflicts with [doc] which says Y. Either we change the doc or we change the proposal — which is it?"* The docs are hypotheses too, but contradictions must be resolved explicitly, not ignored.

### Monorepo awareness

This is a monorepo with multiple crates. When the user describes a problem, determine which crate it belongs to using the Project Overview table in the repo-root `CLAUDE.md` (the one place the crate list is maintained).

If the work spans multiple crates, note this explicitly — the specs may need to live in more than one place, or the problem needs to be decomposed.

## Process

Work through these phases in order; each one ends with the user's explicit confirmation before the next begins.

---

### Phase 1: Problem Statement

Start by stating what you understand the problem to be, based on the user's input and the product docs you've read. Then ask the user to confirm or correct.

Questions to settle:

- **Who has this problem?** If the answer is vague ("users"), push for specifics.
- **What happens today without this?** If nobody can articulate the pain, the problem may not exist.
- **Why now?** What changed that makes this worth doing?
- **What is NOT the problem?** Explicitly scope out adjacent concerns.
- **What would "solved" look like?** Not the solution — the outcome.

Bring your own perspective from the docs. If the docs already describe this problem area, say what you think the answer is and ask the user to push back. This is a dialogue, not an interrogation.

**Exit criteria:** Both you and the user can state the problem in 1-2 sentences, and you both agree on it.

---

### Phase 2: Behavior Discovery

Now explore the expected behavior. This is where Gherkin scenarios emerge.

Start by proposing scenarios yourself based on what you've learned from the docs and Phase 1. Then iterate with the user.

Explore:

- **What does the happy path look like?** Walk through it step by step.
- **What are the edge cases?** What happens when things go wrong?
- **What are the boundaries?** What is explicitly out of scope?
- **What existing behavior must NOT change?** Protect invariants.

For each behavior, draft a Gherkin scenario and present it. Ask: **"Is this exactly right? What did I miss?"**

Challenge weak scenarios:

- If a scenario is too vague: *"This scenario doesn't tell me what actually happens. What does the user see?"*
- If a scenario tests implementation rather than behavior: *"This describes how it works, not what the user experiences. Reframe it."*
- If there's an obvious edge case missing: *"What happens when X? I don't see a scenario for that."*

Use glossary terms in scenarios. If the user uses a non-canonical term, gently correct: *"The docs call this 'eject', not 'detach' — let's use the canonical term."*

Scenarios are hypotheses. If the user presents a strong argument that a scenario is wrong, change it. If you think a scenario the user wants is wrong, say so and explain why.

**Exit criteria:** A set of Gherkin scenarios that both you and the user agree fully describe the expected behavior. The user has explicitly confirmed: "Yes, if all these scenarios pass, the feature is done."

---

### Phase 3: Solution Direction

Now choose *how* we're going to solve it — at the direction level, not the design level.

The output of this phase is: an agreed approach, a short list of alternatives considered (with reasons for rejecting them), and any constraints or assumptions that shaped the choice.

A good "approach" statement answers:

- **What's the shape of the solution?** ("Surface a new typed error from the session layer and let the CLI render it" — not "add a `SessionExpired` variant to `enum SessionError` in `session.rs`".)
- **Where does the work happen?** Which crate/layer owns the change.
- **What does NOT change?** Which existing interfaces, contracts, or behaviors are preserved.

Start by proposing 2-3 plausible approaches yourself, grounded in the product docs and the crate's existing structure. Then narrow down with the user.

Challenge weak thinking:

- **"Why this approach and not the obvious alternative?"** If the user jumps to a solution, make them articulate what they rejected and why.
- **"What are we assuming?"** Surface load-bearing assumptions. If an assumption turns out to be wrong, the approach changes.
- **"What's the cost?"** Every approach has a cost — new surface area, migration, performance, complexity. Name it.
- **"Does this conflict with the docs?"** Re-check the chosen direction against the product docs. Flag conflicts explicitly.

Keep this at direction level. Types, files, and function signatures belong in the Phase 5 plan.

Approaches are hypotheses. If a stronger argument emerges during implementation, the approach can change — but the change must be explicit, not a silent drift.

**Exit criteria:**

- An agreed one-paragraph approach statement.
- 1-3 rejected alternatives with one-line reasons for each.
- Any load-bearing assumptions or constraints written down.
- The user has explicitly confirmed: "Yes, this is the direction."

---

### Phase 4: Alignment Check

Verify the thinking against what is already written down — the docs and the source. This is the last gate before a `.feature` file gets written.

Docs:

1. **Scenarios vs product docs.** Do the agreed scenarios conflict with `docs/` or the Product Vision in the repo-root `CLAUDE.md`?
2. **Approach vs product docs.** Does the chosen direction fit what LNS is documented to be and do? An approach that drifts outside the product shape is a warning sign.
3. **Ownership boundary.** Do the scenarios stay within what LNS owns? If this is about how agents *think and act* rather than where they *run and what they can reach*, it probably doesn't belong here.
4. **Terminology.** Names in the scenarios and approach match the canonical terminology in `docs/` and the repo-root `CLAUDE.md`. Correct non-canonical terms.

Source:

5. **Existing specs.** Read the target crate's feature files — `tests/behaviours/*.feature` (Layer 2) and, if relevant, `e2e/*.feature` (Layer 1). Overlapping scenarios must be reconciled (merge, rename, or replace), not silently duplicated.
6. **Approach vs current source.** Open the crate. Does the agreed approach fit the code as it exists today?
7. **Re-verify assumptions.** Walk through each load-bearing assumption from Phase 3. Can you confirm it from the code? An assumption that looked safe may be wrong in practice — if so, the approach needs revisiting.

If a conflict surfaces, resolve it now: either the docs are wrong, or the plan is. Don't write a `.feature` file on top of an unresolved contradiction. If the approach no longer holds, go back to Phase 3 rather than papering over it.

Then present one short summary — the conflicts and reconciliations you found, plus the scenario titles, target crate, the explicit non-goals, and any docs that need updating if this work lands. Do not restate the problem, approach, and alternatives that the user has just agreed to.

Ask: **"Are we aligned? Should I write the `.feature` file and move to technical planning?"**

**Exit criteria:** No outstanding conflicts, and the user has explicitly confirmed.

---

### Phase 5: Technical Planning

Only after the user confirms alignment:

1. Write the agreed Gherkin scenarios to the appropriate `.feature` file, following the repo's **Test layers** convention (see the repo-root `CLAUDE.md`). Default to **Layer 2** behavioural specs at `crates/<crate>/tests/behaviours/` — that's the primary coverage-bearing layer for user-visible behavior. Only put a scenario in **Layer 1** (`crates/<crate>/e2e/`) if it genuinely requires real binaries / a real microVM / real I/O. Match the file naming and style of existing `.feature` files in that directory; if a suitable existing file exists, append to it, otherwise create a new one (and add a step definition stub under `tests/behaviours/steps/` if the phrasing is new).
2. Enter plan mode for technical implementation planning, starting from the agreed approach — not from a blank page. The plan refines the approach into concrete types, files, and steps. It should not reopen the "what direction" question; if it needs to, that's a scope drift and we go back to Phase 3.

The Gherkin file is the contract. Implementation must satisfy it, nothing more, nothing less.

If the change has non-obvious unit-test scope (e.g. internal helpers, error-mapping logic not visible at the Gherkin level — Layer 3 territory), note it in the plan — but Gherkin scenarios remain the acceptance bar. Each plan step should fold in its own failing-test-first work rather than deferring all tests to a tail-end step.

---

## Rules

- **Solution direction belongs in Phase 3.** While the problem and behavior are still being settled (Phases 1-2), keep proposals to scenarios, not approaches.
- **Preserve the agreed Gherkin exactly.** Do not rephrase scenarios when writing the file.
- **Use canonical terminology** from `docs/` and the repo-root `CLAUDE.md`, and correct non-canonical terms when you see them.
- **Say each thing once.** Confirm at each phase exit and move on; do not re-summarize agreed material.
- **Match your intensity to the user's preparation.** Someone who has already thought it through needs confirmation, not interrogation; someone winging it needs the harder questions.
