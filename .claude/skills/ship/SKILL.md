---
name: ship
description: >
  Pipeline dispatcher — detect where the current work stands (intake →
  spec/plan → implement slices → self-review → PR → respond to review) from
  observable state and drive it forward by invoking the right skill
  (/problem-first, /problem-first-impl, /green, /code-review, /create-pr,
  /review-comments), pausing at every human gate. Use when the user runs
  `/ship`, optionally with an issue number or a one-line idea. Do NOT
  auto-invoke.
---

# Ship — pipeline dispatcher

You contain zero domain logic. Your only job is: figure out where the work
stands, invoke the right sub-skill via the Skill tool, and keep going until
the next human gate or until the pipeline is done. The sub-skills own all
behavior and all human checkpoints — you inherit their pauses, you never add
shortcuts around them.

One invocation does as much as the next human gate allows. Across a large
effort the user runs `/ship` many times; each time you resume from observable
state, never from conversation memory.

## Arguments

- *(empty)* — detect phase from the current branch and plan file.
- Issue number / `#124` / issue URL — drive the pipeline from that issue
  (`/problem-first-impl` Mode B).
- Free text — a new idea; start at intake with it as the problem seed.

## The plan file

The durable plan lives at `.claude/plans/<current-branch>.md` — local,
per-checkout, no GitHub required. Before writing it the first time, ensure
`.claude/plans/` is listed in `.git/info/exclude` (append the line if
missing; never touch the repo's `.gitignore` for this).

Format:

```markdown
# <branch> — plan
Source: <issue ref or "session intake">

- [ ] 1. <slice: scenarios to activate, files to touch>
- [ ] 2. ...
- [ ] Self-review (/code-review)
- [ ] Open PR (/create-pr)        <- include only when gh/GitHub is in play
```

Implementation boxes are ticked by `/green` on each successful close; you
tick the Self-review and Open PR boxes yourself when those phases complete.

## State detection

Gather (in parallel, cheaply): current branch; existence and contents of
`.claude/plans/<branch>.md`; remaining `@todo` scenarios
(`grep -rl '@todo' crates/*/tests/behaviours/ 2>/dev/null`); `git status` and
`git log origin/main..HEAD --oneline`; `gh pr view --json state,reviews`
(optional — if `gh` fails or is offline, GitHub phases are simply skipped).

First matching row wins:

| Observable state | Phase | Action |
|---|---|---|
| Free-text args, or no plan file + no agreed problem in conversation + no issue arg | Intake | Invoke `/problem-first`. When intake concludes and the user is NOT implementing now, suggest `/problem-first-issue` (GitHub only) and stop. |
| Issue arg given, or intake agreed and proceeding; no plan file for this branch | Spec + plan | Invoke `/problem-first-impl` (Mode B when driven from an issue). After the plan is approved: if on `main`, create a feature branch named for the scope; then write the plan file from the approved plan. |
| Plan file has unchecked implementation boxes | Implement | For the first unchecked step: failing-test-first per the repo Workflow (activate the step's scenarios, watch them fail, implement), then invoke `/green`. On green (box ticked), continue to the next unchecked step without pausing. Stop only when `/green` escalates or reports stuck. |
| All implementation boxes ticked; Self-review box unchecked | Review | Invoke `/code-review` (self-review mode). If it produces fixes, apply them and invoke `/green` after. On "Ship it" / all findings resolved, tick the box. |
| Self-review ticked; Open PR box present and unchecked; `gh` available | Package | Invoke `/create-pr`. Tick the box. |
| PR open with unresolved review comments | Respond | Invoke `/review-comments`. Every fix it applies goes through `/green` before replying/resolving. |
| Everything ticked, no unresolved comments | Done | Report the pipeline state and stop. |

## Rules

- **Never bypass a red gate.** If `/green` stops without green, you stop too
  and surface its report — do not route around it to reach the next phase.
- **Inherit every human gate.** `/problem-first` convergence, `-impl`'s
  Phase 4 confirmation and plan-mode approval, `/green` escalations — when a
  sub-skill needs the user, end the turn there; the next `/ship` resumes.
- **GitHub is optional.** `gh` unavailable → skip the park/package/respond
  rows entirely; the plan file is the source of truth either way.
- **Don't re-plan half-done work.** A plan file with ticked boxes means the
  plan was approved; resume at the first unchecked step. Only return to
  planning if the user says the direction changed — and direction changes go
  back to `/problem-first`, not to silent drift.
- **Add nothing to the sub-skills' behavior.** You sequence them; you don't
  extend, abbreviate, or second-guess them.
