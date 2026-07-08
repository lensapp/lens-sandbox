---
name: ship
description: >
  Drive the problem-first pipeline forward — detect the current phase
  (intake, plan, implement, review, PR) from observable state and invoke the
  right skill, pausing at every human gate. Use only when the user runs
  `/ship`, optionally with an issue number or a one-line idea.
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

## Isolated worktree

Do NOT run the pipeline in the user's working directory — they may be in the
middle of something else, and `/ship` can start from a one-line idea while
they sit on `main`. When the Spec + plan phase creates the feature branch,
create a worktree for it (same pattern as `/code-review`):

```bash
MAIN_ROOT="$(git worktree list | head -1 | awk '{print $1}')"
SHIP_DIR="$MAIN_ROOT/.ship/<branch>"
mkdir -p "$MAIN_ROOT/.ship"
git worktree add "$SHIP_DIR" -b <branch> origin/main
```

Ensure `.ship/` is listed in `.git/info/exclude`. Every subsequent phase —
implementation, `/green`, `make e2e`, `/code-review`, `/create-pr` — runs
inside `$SHIP_DIR`. On resume, if the plan file names a branch whose worktree
is missing, re-add it (`git worktree add "$SHIP_DIR" <branch>`).

The worktree builds its own `target/` — real disk on this host. `/green`'s
disk guard applies per-worktree.

**Cleanup** happens on PR terminal state, not at first "everything ticked" —
an open PR can still receive review comments, and the Respond phase needs the
worktree. When the Done row runs, check `gh pr view --json state`:

- `MERGED` or `CLOSED` → `git worktree remove "$SHIP_DIR"` (reclaims its
  `target/`), `git branch -d <branch>`, delete the plan file, and
  `git worktree prune`.
- `OPEN` → keep everything; report "awaiting review/merge — rerun /ship
  after merge to clean up".
- No PR / no GitHub → there is no merge event to observe; ask the user
  before removing anything.

## The plan file

The durable plan lives in the **main checkout** at
`<main-root>/.claude/plans/<branch>.md` (resolve `<main-root>` via
`git worktree list | head -1` so the path is the same from inside a `.ship/`
worktree) — local, no GitHub required, and it survives worktree removal.
Before writing it the first time, ensure `.claude/plans/` is listed in
`.git/info/exclude` (shared by all worktrees; never touch the repo's
`.gitignore` for this).

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

When invoked from `main`, first look for in-flight work: plan files under
`<main-root>/.claude/plans/` with unchecked boxes. Exactly one → resume that
branch's worktree. Several → ask which. None → this is new work.

First matching row wins:

| Observable state | Phase | Action |
|---|---|---|
| Free-text args, or no plan file + no agreed problem in conversation + no issue arg | Intake | Invoke `/problem-first`. When intake concludes and the user is NOT implementing now, suggest `/problem-first-issue` (GitHub only) and stop. |
| Issue arg given, or intake agreed and proceeding; no plan file for this branch | Spec + plan | Invoke `/problem-first-impl` (Mode B when driven from an issue). After the plan is approved: create the feature branch **in a `.ship/` worktree** (see Isolated worktree), write the plan file, and run all further phases inside it. |
| Plan file has unchecked implementation boxes | Implement | For the first unchecked step: failing-test-first per the repo Workflow (activate the step's scenarios, watch them fail, implement), then invoke `/green`. On green (box ticked), continue to the next unchecked step without pausing. Stop only when `/green` escalates or reports stuck. |
| All implementation boxes ticked; Self-review box unchecked | Review | Run `make e2e` once (Layer 1 wiring — CI's test job runs it, `/green` deliberately doesn't); fix failures via `/green`. Then invoke `/code-review` (self-review mode). If it produces fixes, apply them and invoke `/green` after. On "Ship it" / all findings resolved, tick the box. If the branch touches VM/session plumbing, remind the user of the manual checks: `make e2e-microvm` (virt-capable host) and the `lns run -it` smoke test (`expect -f crates/lns-cli/tests/smoke/interactive-shell.exp`). |
| Self-review ticked; Open PR box present and unchecked; `gh` available | Package | Invoke `/create-pr`. Tick the box. |
| PR open with unresolved review comments | Respond | Invoke `/review-comments`. Every fix it applies goes through `/green` before replying/resolving. |
| Everything ticked, no unresolved comments | Done | Check PR state and run the cleanup policy (see Isolated worktree): merged/closed → remove worktree + branch + plan file; open → report "awaiting merge" and keep everything. |

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
