---
name: create-pr
description: >
  Create a pull request for the current branch. Use when user asks to:
  (1) create a PR,
  (2) open a pull request,
  (3) submit changes for review.
  Handles branch pushing, PR creation with gh CLI, and conventional title formatting.
model: sonnet
---

# Creating a Pull Request

## Modes

This skill supports two modes:

### A. Full branch PR (default)

The entire current branch becomes the PR. Used when the user says "create a PR" without specifying a commit.

### B. Single commit PR (cherry-pick)

The user wants to create a PR for a specific commit while continuing work on the current branch. Used when the user says things like "create a PR for this commit", "create a PR for the last commit", or references a specific commit.

**Single commit workflow:**

1. Note the current branch name and the target commit hash
2. Fetch latest and create a new branch from origin/main:
   ```bash
   git fetch origin main
   git checkout -b <branch-name> origin/main
   ```
3. Cherry-pick the commit:
   ```bash
   git cherry-pick <commit-hash>
   ```
4. Push, create the PR (follow steps 2-6 below)
5. Return to the original branch:
   ```bash
   git checkout <original-branch>
   ```
6. The cherry-picked commit stays on the original branch too - that's fine, git handles this during merge

## Prerequisites

- For full branch PR: current branch must NOT be `main`, and there must be commits ahead of the base branch
- For single commit PR: the target commit must exist

## Steps (Full Branch PR)

### 1. Gather Context

Run these in parallel:

```bash
# Fetch latest origin/main (local main may be stale)
git fetch origin main

# Check current branch and remote tracking
git branch --show-current
git status

# See all commits that will be in the PR (from divergence point)
git log origin/main..HEAD --oneline

# See full diff against base
git diff origin/main...HEAD --stat
```

### 2. Push Branch

If the branch has no upstream or is ahead of remote:

```bash
git push -u origin HEAD
```

### 3. Determine PR Title

Follow conventional commits format for the PR title:

- `feat(scope): Description` - New feature
- `fix(scope): Description` - Bug fix
- `chore(scope): Description` - Internal change, refactoring
- `refactor(scope): Description` - Code restructuring
- `test(scope): Description` - Test additions/changes
- `ci(scope): Description` - CI/CD changes

Rules:
- Sentence-case subject ("Add feature", not "add feature")
- Imperative mood ("Add", "Fix", "Remove" - not "Added", "Fixed")
- Use scope from the business/feature area (e.g., "sandbox", "cli", "ui", "auth")
- If multiple commits with mixed types, use the most significant prefix
- If a single commit, reuse its message as the PR title

### 4. Write PR Body

Scale the description to match the complexity of the changes. A one-commit typo fix needs one sentence. A multi-commit feature needs a thorough write-up.

#### Summary

Always start with a `## Summary` section. **Lead with the problem being solved** — what was broken, missing, or inadequate — then explain how this PR addresses it.

For simple changes, 1-3 bullet points suffice. For larger changes, open with a short paragraph framing the problem, then break the solution into **numbered subsections** — one per logical change or concept introduced.

Within subsections, dive deeper only where the solution isn't obvious from the problem statement:
- **Code blocks** showing before/after, config shapes, or CLI examples
- **Tables** comparing options, scopes, or states
- **Inline code** for file paths, commands, flags, and field names
- **Diagrams of flows** when the change involves multi-step interactions

The summary should answer: **what problem does this solve, and how?** Give reviewers enough context to understand the design without reading every line of diff, but don't over-explain straightforward changes.

#### Breaking Changes

If the PR introduces breaking changes (removed fields, renamed files, changed behavior, incompatible config formats), add a `## Breaking changes` section explaining what breaks and what users need to do.

#### Docs

If the PR updates documentation, add a short `## Docs` section noting which docs changed and why.

#### Screenshots / Recordings (for features and bug fixes)

For `feat` and `fix` PRs, include a screenshots/recordings section with placeholders for the user to fill in. These are not needed for `chore` PRs.

For **bug fixes**, use before/after format:

```markdown
## Screenshots
### Before
<!-- Add screenshot or recording showing the bug -->

### After
<!-- Add screenshot or recording showing the fix -->
```

For **features**, show the new functionality:

```markdown
## Screenshots
<!-- Add screenshot or recording demonstrating the new feature -->
```

After creating the PR, remind the user to add the screenshots/recordings.

#### Test Instructions

Include a `## Test instructions` section with concrete, scenario-based steps that a reviewer can follow to verify the changes. Each step should describe a specific scenario with the action to take and the expected outcome.

- Cover the **happy path**, relevant **edge cases**, and **error scenarios**
- Use actual CLI commands, API calls, or UI actions — not abstract descriptions
- State the **expected result** for each step so the reviewer knows what "pass" looks like
- Number each scenario as a self-contained verification step
- Omit this section only for trivial changes (typo fixes, comment-only changes, CI config tweaks)

Example:

```markdown
## Test instructions

1. Fresh directory, first run: `lns run python:3.12 -- python -V` prompts for posture, creates `.lns/settings.json` with the chosen preset.
2. Same directory, second run of the same image: no prompts; uses the recorded posture.
3. Trigger a denial: `lns run python:3.12 -- curl https://example.com`; observe the OS notification with `Allow once / Always / Deny`. Click `Always` and verify the host lands in `.lns/settings.local.json`.
4. Headless review: `lns policy review --watch` in a second terminal mirrors the notification queue and accepts inline.
5. `lns policy preset high` rewrites the team baseline; existing `allow`/`deny` rules in the project layer survive.
```

### 5. Create PR

```bash
gh pr create --title "feat(scope): Title here" --body "$(cat <<'EOF'
## Summary

<narrative or bullet points — scale to complexity>

### 1. <First logical change>
<explanation with code blocks, tables, or examples as needed>

### 2. <Second logical change>
...

## Breaking changes
<if applicable>

## Docs
<if docs were updated>

## Test instructions
1. <scenario + expected outcome>
EOF
)"
```

### 6. Report Back

Show the user the PR URL returned by `gh pr create`.

## Flags

| Flag | When to Use |
|------|-------------|
| `--draft` | User says "draft PR" or changes are work-in-progress |
| `--base <branch>` | Target branch is not the default (main) |
| `--assignee @me` | User wants to self-assign |

## Don't

- **Don't create PR if on main** - ask user to create a branch first
- **Don't force push** - just regular push
- **Don't add labels** unless the user explicitly asks
- **Don't auto-merge** - just create the PR
- **Don't add "Generated with Claude Code"** or any AI attribution to the PR body
- **Don't add Co-Authored-By trailers** to commits or PR body
