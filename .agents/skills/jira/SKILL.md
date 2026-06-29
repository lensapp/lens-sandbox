---
name: jira
description: >
  Create a Jira issue in LNS. Use when user asks to:
  (1) create a Jira issue or ticket,
  (2) "/jira" with arguments.
  Handles assignment, status transitions, and PR linking.
model: sonnet
---

# Create Jira Issue

Create a LNS Jira issue with optional assignment and status transition.

## Argument Parsing

Arguments are comma-separated, order doesn't matter. Parse each segment:

| Pattern | Meaning | Examples |
|---------|---------|----------|
| `me` | Assign to current user | `me` |
| `assign <name>` | Assign to named person | `assign Janne` |
| `in review` | Transition to In Review | `in review` |
| `in progress` | Transition to In Progress | `in progress` |
| `done` | Transition to Done | `done` |
| `to do` | Transition to To Do | `to do` |
| `bug` | Issue type: Bug | `bug` |
| `story` | Issue type: Story | `story` |
| `epic` | Issue type: Epic | `epic` |
| Everything else | Extra context for summary/description | `fix the notification credits` |

**Defaults:** Issue type = Task, no assignee, status = To Do (no transition).

## Constants

- **Cloud ID:** `b2bbb94e-2efa-4b0c-aab6-1195ff16dab7`
- **Project:** `LNS`

## Steps

### 1. Parse Arguments

Split the ARGUMENTS string by commas, trim whitespace, and classify each segment using the table above. Matching is case-insensitive.

### 2. Gather Context (parallel)

Run these in parallel to collect information:

**a) PR detection** - check if current branch has a PR:
```bash
gh pr view --json url,title,body,number 2>/dev/null || echo "NO_PR"
```

**b) Git context** - get branch name and recent commits for summary generation:
```bash
git branch --show-current
git log --oneline -5
```

**c) Assignee lookup** (only if `me` or `assign <name>` was used):

- For `me`: call `atlassianUserInfo` to get `account_id`
- For `assign <name>`: call `lookupJiraAccountId` with the name as search string, use first result

### 3. Generate Summary and Description

Use conversation context and git context to generate:

- **Summary** (issue title): concise, imperative mood (e.g., "Credit PR author instead of merger in broken-master-fixed notification")

**For Tasks and Stories:**

- **Description**: 1-3 sentences focusing on **what** the problem is and **why** it matters. Do NOT describe the solution - the PR contains that. Do NOT include a link to the PR in the description.

**For Bugs — use this structured template:**

```
**Short Description**
[1-2 sentences: what is broken and under what conditions]

**Steps to Reproduce**
1. [First step]
2. [Second step]
3. [Continue until the bug manifests]

**Expected Behavior**
[What should happen when following the steps above]

**Actual Behavior**
[What actually happens — include error messages if available, truncated to 2000 chars]

**Impact**
[Who is affected and how severely — e.g. "Blocks all users from running sandboxed agents"]
```

**Bug description rules:**
- Use bold text (`**Header**`) for section headers, NOT markdown headings (`##`) — headings render poorly in Jira
- Do NOT set Priority, Technical Impact, or Business Urgency — the triage team handles those
- Truncate error messages to 2000 characters
- Derive Steps to Reproduce from git context (what code path was affected) or user-provided context

If the user provided extra context text in arguments, use that to inform the summary/description.

### 4. Create Issue

Call `createJiraIssue` with:
- `cloudId`: `b2bbb94e-2efa-4b0c-aab6-1195ff16dab7`
- `projectKey`: `LNS`
- `issueTypeName`: parsed type or `Task`
- `summary`: generated summary
- `description`: generated description
- `assignee_account_id`: resolved account ID (if assignee was requested)

### 5. Transition Status (if requested)

If a target status was parsed, call `getTransitionsForJiraIssue` for the new issue, then `transitionJiraIssue` with the matching transition ID.

### 6. Link PR to Jira Issue (if PR exists)

If a PR was detected in step 2a, update the PR description to include a Jira link. Append to the existing PR body:

```bash
# Use a temp file to safely handle special characters in the PR body
gh pr view <pr_number> --json body -q .body > /tmp/pr_body.txt
printf '\n\nFixes https://mirantis.jira.com/browse/<issue_key>' >> /tmp/pr_body.txt
gh pr edit <pr_number> --body-file /tmp/pr_body.txt
```

This creates the link from the PR to the Jira issue, not the other way around.

### 7. Report Back

Show the user:
- Issue key and URL: `https://mirantis.jira.com/browse/<key>`
- What was set: assignee, status
- Whether the PR description was updated with the Jira link

## Examples

```
/jira me, in review
-> Task assigned to me, transitioned to In Review, PR updated with Jira link

/jira me, in progress, bug
-> Bug assigned to me, transitioned to In Progress

/jira assign Juho, in progress
-> Task assigned to Juho, transitioned to In Progress

/jira bug, me, sandbox agent fails to start
-> Bug with structured description (Steps to Reproduce, Expected/Actual, Impact)
-> Assigned to me
```

## Don't

- **Don't describe the solution in the description** - focus on what/why, the PR has the how
- **Don't put PR links in the Jira issue** - the link goes from PR -> Jira, not Jira -> PR
- **Don't ask for confirmation** before creating - just create it
- **Don't add labels or components** unless the user explicitly asks
- **Don't use markdown headings (`##`) in bug descriptions** - use bold text (`**Header**`) instead, headings render poorly in Jira
- **Don't set triage fields** (Priority, Technical Impact, Business Urgency) on bugs - the triage team owns those
