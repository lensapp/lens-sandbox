---
name: problem-first-jira
description: >
  Continuation of `/problem-first`. Creates a LNS Jira Story (unassigned,
  in backlog) capturing the agreed problem statement and Gherkin scenarios
  as acceptance criteria. Use when the user explicitly runs
  `/problem-first-jira`, or when they ask to file a ticket for a problem
  they've just worked through via `/problem-first`. Do NOT auto-invoke.
model: sonnet
---

# Spec — Problem-First → Jira

Turn the output of a `/problem-first` session into a LNS backlog Story. The ticket is a handoff artifact: someone else (or the same person later) picks it up, reads the problem + scenarios, and implements against them.

This skill is intentionally narrow. No assignment. No status transitions. No arg parsing. No PR linking. No epic linking. Story in Backlog, that's it.

For the mechanics of calling Jira (cloud ID, `createJiraIssue` signature, markdown-in-Jira conventions), follow `.Codex/skills/jira/SKILL.md`. This skill focuses on content generation and assumes you can reuse the Jira-call details from there.

## Constants

- **Cloud ID:** `b2bbb94e-2efa-4b0c-aab6-1195ff16dab7`
- **Project:** `LNS`
- **Issue type:** `Story`
- **Assignee:** none
- **Status:** default (Backlog / To Do — do not transition)

## Inputs

You need the full output of a `/problem-first` session before creating the ticket. The ticket must carry enough context that whoever picks it up — possibly weeks later, possibly not the person who planned it — can do technical planning and implement without redoing the intake.

Required:

1. A **problem statement** (1-2 sentences).
2. An agreed set of **Gherkin scenarios** describing the expected behavior.
3. An **approach**: a one-paragraph solution direction with the shape of the solution, where the work happens, and what does not change.
4. **Alternatives considered**: 1-3 rejected alternatives with one-line reasons for each.

Optional but include if present:

- **Target package** (e.g. `lns-cli`).
- **Assumptions / constraints** that shaped the approach.
- **Code pointers** (file paths, function names, off-limits modules the filer identified).
- **Non-goals**.
- **Open questions** (product-doc conflicts or unresolved decisions).

### Invocation modes

**Mode A — Continuation in the same session as `/problem-first`.**
If `/problem-first` just ran in this conversation, read the full output (problem, scenarios, approach, alternatives, assumptions, non-goals, open questions) directly from conversation context. Do NOT re-elicit. Briefly confirm back: *"Using the problem, N scenarios, and chosen approach from the `/problem-first` session above — creating the Story now."*

**Mode B — Standalone.**
If there is no prior `/problem-first` output in the session, the user is trying to file a ticket cold. Do NOT fabricate content. Either:

- Ask the user to paste the problem statement, Gherkin scenarios, approach, and alternatives considered, OR
- Suggest: *"This skill works best after `/problem-first` — the ticket needs the problem, scenarios, and chosen approach. Want to run `/problem-first` first, or paste what you already have?"*

Only proceed once at minimum problem + scenarios + approach are present. If approach is missing, do not create the ticket with just scenarios — send the user back to `/problem-first` Phase 3 rather than filing a ticket that can't be picked up.

## Steps

### 1. Gather Inputs

Resolve Mode A vs Mode B as described above. Finalize: problem statement, scenarios, target package (if known), non-goals (if any), open questions (if any).

### 2. Generate Summary

Concise, imperative mood, derived from the problem statement. No ticket prefix, no Jira-speak, no trailing period.

Good: *"Surface expired sandbox sessions to the user"*
Bad: *"[STORY] As a user, I want expired sandbox sessions to be surfaced"*

### 3. Generate Description

Use bold section headers (`**Header**`), NOT markdown `##` headings — `##` renders poorly in Jira. Fenced code blocks are fine and should be used for Gherkin.

The description has these sections, in this order. Omit any section marked *(optional)* if it doesn't apply.

1. `**Problem**` — the agreed 1-2 sentence problem statement.
2. `**Target Package**` *(optional)* — e.g. `lns-cli`. Omit if not yet identified.
3. `**Approach**` — the agreed one-paragraph solution direction from `/problem-first` Phase 3. Shape of the solution, where the work happens, what does not change. Direction, not design.
4. `**Alternatives considered**` — bullet list of 1-3 rejected alternatives with one-line reasons. Omit only if `/problem-first` genuinely produced none (rare — push back before accepting this).
5. `**Assumptions / Constraints**` *(optional)* — bullet list of load-bearing assumptions or constraints that shaped the approach.
6. `**Code Pointers**` *(optional)* — bullet list of file paths, function names, or module boundaries the filer identified during `/problem-first` that give the implementer a head start. Include "off-limits" items here too (files/modules that must NOT be touched). Omit if the filer didn't surface any.
7. `**Acceptance Criteria**` — a short intro sentence followed by a fenced code block tagged `gherkin` containing a single `Feature:` block with all agreed scenarios. Preserve the exact Gherkin text from `/problem-first`.
8. `**Non-goals**` *(optional)* — bullet list of explicit out-of-scope items. May include code-scope boundaries (e.g. "don't touch module X", "component Y is off-limits for this ticket").
9. `**Open Questions**` *(optional)* — bullet list of unresolved questions from Phases 1-3.

Guiding principle: the ticket must be self-contained. Someone opening it cold should understand *the problem*, *what "done" looks like* (Gherkin), *which direction we chose and why* (Approach + Alternatives), and *what we're assuming*. They should not need to find the person who filed it.

Example shape (adapted from a real `/problem-first` session):

    **Problem**
    Users running `lns up` against an expired session see a confusing
    "connection refused" error instead of being told the session is gone.

    **Target Package**
    lns-cli

    **Approach**
    Surface session expiry as a typed error from the session layer and let
    the CLI render a clear "session expired" message before any connection
    attempt is made. The expiry check moves ahead of the connect path; the
    connect path itself is unchanged. No new user-facing config.

    **Code Pointers**
    - `packages/lns-cli/src/session/mod.rs` — session TTL check lives here
    - `packages/lns-cli/src/cli/up.rs` — connect path (off-limits: don't restructure)

    **Alternatives considered**
    - Retry transparently on expiry: rejected — hides a state the user
      needs to act on (re-auth), and masks real connectivity failures.
    - Detect expiry by parsing the connect-refused error: rejected —
      brittle, couples the CLI to transport-level error strings.

    **Assumptions / Constraints**
    - Session TTL is already tracked client-side; no server round-trip
      needed to know a session has expired.
    - Exit code for expired sessions can differ from the generic connect
      failure code without breaking existing scripts (verified: no
      documented contract on that code).

    **Acceptance Criteria**
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

The example above uses 4-space indentation to show the literal description content without nesting fenced code blocks. When you generate the real description passed to `createJiraIssue`, output the content left-aligned (no leading indent) and use real triple-backtick fences around the Gherkin block.

Notes:

- Use one `Feature:` block containing all scenarios. Don't split into multiple fenced blocks unless the scenarios genuinely belong to different feature files.
- Preserve the exact Gherkin text agreed in `/problem-first`. Do not rephrase.
- Do NOT describe a solution. The ticket is a problem + behavior contract, not a design.
- Do NOT add components, priority, or any triage fields. The triage team owns those. The only label permitted is `ready-for-agent`, applied only after the filer confirms agent-readiness in Step 4.

### 4. Create the Issue

Call `createJiraIssue` with:

- `cloudId`: `b2bbb94e-2efa-4b0c-aab6-1195ff16dab7`
- `projectKey`: `LNS`
- `issueTypeName`: `Story`
- `summary`: generated summary
- `description`: generated description
- no `assignee_account_id`

Do not call `getTransitionsForJiraIssue` or `transitionJiraIssue` — leave the ticket in its default state (Backlog / To Do).

After successful creation, ask:

> "Is this ticket ready for an agent to pick up without human guidance? (yes / no)"

- **If yes:** call `editJiraIssue` to add the label `ready-for-agent` to the issue. No other labels.
- **If no (or skipped):** leave the ticket unlabelled. The filer can add `ready-for-agent` later via `/jira` or the Jira UI.

### 5. Report Back

Show the user:

- Issue key and URL: `https://mirantis.jira.com/browse/<key>`
- Summary line
- Scenario count, rejected-alternatives count, target package (if any)
- A reminder: *"Unassigned, in backlog. Carries problem, scenarios, and chosen approach — ready for pickup."*

## Don't

- **Don't auto-invoke.** Only run when the user explicitly asks or uses `/problem-first-jira`.
- **Don't re-run Phases 1-3.** That's `/problem-first`'s job. If inputs are missing, send the user back there.
- **Don't assign or transition the ticket.** The only label permitted is `ready-for-agent`, and only when the filer explicitly confirms agent-readiness in Step 4.
- **Don't link a PR.** There is no PR at this stage.
- **Don't invent Gherkin or an approach.** If the user hasn't agreed them, don't fabricate — send them back to `/problem-first`.
- **Don't file a ticket with only scenarios and no approach.** A ticket the implementer can't pick up is worse than no ticket.
- **Don't descend into technical design.** The Approach is direction, not types or function signatures. Detailed design happens in `/problem-first-impl` and the eventual PR.
- **Don't use markdown `##` headings in the description.** Use `**Header**`.
