---
name: problem-first-issue
description: >
  Continuation of `/problem-first`. Creates a GitHub issue (open,
  unassigned) capturing the agreed problem statement and Gherkin scenarios
  as acceptance criteria. Use when the user explicitly runs
  `/problem-first-issue`, or when they ask to file an issue for a problem
  they've just worked through via `/problem-first`. Do NOT auto-invoke.
model: sonnet
---

# Spec — Problem-First → GitHub Issue

Turn the output of a `/problem-first` session into a GitHub issue. The issue is a handoff artifact: someone else (or the same person later) picks it up, reads the problem + scenarios, and implements against them.

This skill is intentionally narrow. No assignment. No labels. No milestone. No project board. An open, unassigned issue against the repo's `origin` remote, that's it.

The issue is created with the `gh` CLI. GitHub renders Markdown natively, so the body uses normal Markdown headings (`##`) and fenced code blocks.

## Inputs

You need the full output of a `/problem-first` session before creating the issue. The issue must carry enough context that whoever picks it up — possibly weeks later, possibly not the person who planned it — can do technical planning and implement without redoing the intake.

Required:

1. A **problem statement** (1-2 sentences).
2. An agreed set of **Gherkin scenarios** describing the expected behavior.
3. An **approach**: a one-paragraph solution direction with the shape of the solution, where the work happens, and what does not change.
4. **Alternatives considered**: 1-3 rejected alternatives with one-line reasons for each.

Optional but include if present:

- **Target crate** (e.g. `lns-cli`).
- **Assumptions / constraints** that shaped the approach.
- **Code pointers** (file paths, function names, off-limits modules the filer identified).
- **Non-goals**.
- **Open questions** (product-doc conflicts or unresolved decisions).

### Invocation modes

**Mode A — Continuation in the same session as `/problem-first`.**
If `/problem-first` just ran in this conversation, read the full output (problem, scenarios, approach, alternatives, assumptions, non-goals, open questions) directly from conversation context. Do NOT re-elicit. Briefly confirm back: *"Using the problem, N scenarios, and chosen approach from the `/problem-first` session above — preparing the issue now."*

**Mode B — Standalone.**
If there is no prior `/problem-first` output in the session, the user is trying to file an issue cold. Do NOT fabricate content. Either:

- Ask the user to paste the problem statement, Gherkin scenarios, approach, and alternatives considered, OR
- Suggest: *"This skill works best after `/problem-first` — the issue needs the problem, scenarios, and chosen approach. Want to run `/problem-first` first, or paste what you already have?"*

Only proceed once at minimum problem + scenarios + approach are present. If approach is missing, do not create the issue with just scenarios — send the user back to `/problem-first` Phase 3 rather than filing an issue that can't be picked up.

## Steps

### 1. Gather Inputs

Resolve Mode A vs Mode B as described above. Finalize: problem statement, scenarios, target crate (if known), non-goals (if any), open questions (if any).

### 2. Generate Title

Concise, imperative mood, derived from the problem statement. No issue-number prefix, no "[STORY]" tag, no trailing period.

Good: *"Surface expired sandbox sessions to the user"*
Bad: *"[STORY] As a user, I want expired sandbox sessions to be surfaced"*

### 3. Generate Body

GitHub renders Markdown, so use `##` section headings and fenced code blocks.

The body has these sections, in this order. Omit any section marked *(optional)* if it doesn't apply.

1. `## Problem` — the agreed 1-2 sentence problem statement.
2. `## Target Crate` *(optional)* — e.g. `lns-cli`. Omit if not yet identified.
3. `## Approach` — the agreed one-paragraph solution direction from `/problem-first` Phase 3. Shape of the solution, where the work happens, what does not change. Direction, not design.
4. `## Alternatives considered` — bullet list of 1-3 rejected alternatives with one-line reasons. Omit only if `/problem-first` genuinely produced none (rare — push back before accepting this).
5. `## Assumptions / Constraints` *(optional)* — bullet list of load-bearing assumptions or constraints that shaped the approach.
6. `## Code Pointers` *(optional)* — bullet list of file paths, function names, or module boundaries the filer identified during `/problem-first` that give the implementer a head start. Include "off-limits" items here too (files/modules that must NOT be touched). Omit if the filer didn't surface any.
7. `## Acceptance Criteria` — a short intro sentence followed by a fenced code block tagged `gherkin` containing a single `Feature:` block with all agreed scenarios. Preserve the exact Gherkin text from `/problem-first`.
8. `## Non-goals` *(optional)* — bullet list of explicit out-of-scope items. May include code-scope boundaries (e.g. "don't touch module X", "component Y is off-limits for this issue").
9. `## Open Questions` *(optional)* — bullet list of unresolved questions from Phases 1-3.

Guiding principle: the issue must be self-contained. Someone opening it cold should understand *the problem*, *what "done" looks like* (Gherkin), *which direction we chose and why* (Approach + Alternatives), and *what we're assuming*. They should not need to find the person who filed it.

Example shape (adapted from a real `/problem-first` session):

    ## Problem
    Users running `lns up` against an expired session see a confusing
    "connection refused" error instead of being told the session is gone.

    ## Target Crate
    lns-cli

    ## Approach
    Surface session expiry as a typed error from the session layer and let
    the CLI render a clear "session expired" message before any connection
    attempt is made. The expiry check moves ahead of the connect path; the
    connect path itself is unchanged. No new user-facing config.

    ## Alternatives considered
    - Retry transparently on expiry: rejected — hides a state the user
      needs to act on (re-auth), and masks real connectivity failures.
    - Detect expiry by parsing the connect-refused error: rejected —
      brittle, couples the CLI to transport-level error strings.

    ## Assumptions / Constraints
    - Session TTL is already tracked client-side; no server round-trip
      needed to know a session has expired.
    - Exit code for expired sessions can differ from the generic connect
      failure code without breaking existing scripts.

    ## Code Pointers
    - `crates/lns-cli/src/session/mod.rs` — session TTL check lives here
    - `crates/lns-cli/src/cli/up.rs` — connect path (off-limits: don't restructure)

    ## Acceptance Criteria
    The following Gherkin scenarios describe the expected behavior. If all
    scenarios pass, the feature is done.

    ```gherkin
    Feature: Expired session reporting

      Scenario: User runs lns up against an expired session
        Given the user has a session whose TTL has elapsed
        When the user runs `lns up`
        Then the CLI reports "session expired" and exits non-zero
        And no connection attempt is made

      Scenario: User runs lns up against a live session
        Given the user has a session within its TTL
        When the user runs `lns up`
        Then the CLI connects normally
    ```

The example above uses 4-space indentation to show the literal body content without nesting fenced code blocks. When you generate the real body written to the temp file, output the content left-aligned (no leading indent) and use real triple-backtick fences around the Gherkin block.

Notes:

- Use one `Feature:` block containing all scenarios. Don't split into multiple fenced blocks unless the scenarios genuinely belong to different feature files.
- Preserve the exact Gherkin text agreed in `/problem-first`. Do not rephrase.
- Do NOT describe a solution beyond the agreed direction. The issue is a problem + behavior contract, not a design.
- Do NOT add labels, milestones, assignees, or projects. The triage team owns those.

### 4. Create the Issue

This files a public artifact, so confirm before creating. Show the user the generated **title** and **body** and ask for a quick go-ahead (in Mode A the content is already agreed, so this is a final glance, not a re-litigation).

On confirmation, write the body to a temp file — this avoids shell-quoting hazards around the Gherkin fences and backticks — then create the issue:

```bash
cat > /tmp/problem-first-issue.md <<'EOF'
<generated body, left-aligned, with real ``` fences>
EOF
gh issue create --title "<generated title>" --body-file /tmp/problem-first-issue.md
```

Do NOT pass `--assignee`, `--label`, `--milestone`, or `--project`. Leave the issue open and unassigned. `gh issue create` prints the new issue's URL on success — capture it for the report.

### 5. Report Back

Show the user:

- Issue number and URL (from `gh issue create` output).
- Title line.
- Scenario count, rejected-alternatives count, target crate (if any).
- A reminder: *"Open and unassigned. Carries problem, scenarios, and chosen approach — ready for pickup."*

## Don't

- **Don't auto-invoke.** Only run when the user explicitly asks or uses `/problem-first-issue`.
- **Don't re-run Phases 1-3.** That's `/problem-first`'s job. If inputs are missing, send the user back there.
- **Don't assign, label, milestone, or add to a project.** An open, unassigned issue is the whole output.
- **Don't link a PR.** There is no PR at this stage.
- **Don't invent Gherkin or an approach.** If the user hasn't agreed them, don't fabricate — send them back to `/problem-first`.
- **Don't file an issue with only scenarios and no approach.** An issue the implementer can't pick up is worse than no issue.
- **Don't descend into technical design.** The Approach is direction, not types or function signatures. Detailed design happens in `/problem-first-impl` and the eventual PR.
