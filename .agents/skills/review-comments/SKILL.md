---
name: review-comments
description: "Handle PR review comments interactively — fetch, triage, present, fix, reply, and resolve. Use this skill whenever the user wants to address PR review comments, fix review feedback, handle reviewer suggestions, respond to code review, resolve review threads, or work through pull request feedback. Triggers on 'fix review comments', 'address review comments', 'handle review feedback', 'resolve review threads', 'review comments', 'what comments are on my PR', 'fix pr feedback', or any mention of working through pull request review feedback."
---

# PR Comments

You help the user work through pull request review comments efficiently. Your job is to fetch all comments, understand them in the context of the PR, triage by severity, present a clear overview, and then guide the user through addressing each one — whether that means fixing the code or crafting a reply.

Quality matters. When you fix something, fix it properly — with tests, just as if you were writing the code from scratch. A reviewer asked for a change because they care about the codebase; honor that by doing the fix right.

## Phase 1: Identify the PR

`$ARGUMENTS` may contain a PR number, URL, or `owner/repo#number`. If empty, detect from the current branch:

```bash
gh pr view --json number,url,title,headRefName,baseRefName 2>/dev/null
```

If no PR is found, ask the user which PR to work on.

Store these for later: `PR_NUMBER`, `PR_URL`, `OWNER`, `REPO`, `HEAD_REF`, `BASE_REF`.

Derive `OWNER` and `REPO` from `gh repo view --json owner,name` or by parsing the PR URL.

## Phase 2: Fetch everything

Fetch all data in parallel:

### 2.1 Review threads (primary source — gives threads, resolution status, and all comments)

```bash
gh api graphql -f query='
  query($owner: String!, $repo: String!, $number: Int!) {
    repository(owner: $owner, name: $repo) {
      pullRequest(number: $number) {
        author { login }
        reviewThreads(first: 100) {
          nodes {
            id
            isResolved
            comments(first: 50) {
              nodes {
                id
                databaseId
                body
                author { login }
                path
                line
                originalLine
                diffHunk
              }
            }
          }
        }
      }
    }
  }
' -f owner="$OWNER" -f repo="$REPO" -F number="$PR_NUMBER"
```

This is the single source of truth for inline review comments. It gives you:
- Thread-level `id` (needed for resolving via GraphQL mutation)
- Comment-level `databaseId` (needed for `in_reply_to` when posting replies via REST)
- Resolution status per thread
- The full comment body, author, file path, and diff context

### 2.2 Issue comments (general PR conversation)

```bash
gh api repos/{owner}/{repo}/issues/{number}/comments --paginate \
  --jq '.[] | {id, body, user: .user.login, created_at}'
```

These are top-level PR conversation comments (not tied to specific code lines). Include them in the overview since reviewers sometimes leave important feedback here.

### 2.3 PR diff

```bash
gh pr diff {number} --repo {owner}/{repo}
```

### 2.4 Filter to actionable comments

- Keep only threads where `isResolved` is `false`
- Discard comments authored by the PR author (self-comments are usually notes, not review feedback) unless they contain a question
- Note the PR author from the `pullRequest.author.login` field to identify self-comments

## Phase 3: Triage and assess

Now you have all unresolved comments. Before showing anything to the user, do the analytical work:

### 3.1 Read the relevant code

For each comment, read the file it references. Understand what the reviewer is asking for and whether the ask is straightforward or requires deeper thought.

### 3.2 Classify each comment

Assign each comment a category:

| Category | Meaning |
|----------|---------|
| **Critical** | Bugs, security issues, broken logic, data loss risks |
| **Should-fix** | Design issues, missing error handling, test gaps, naming problems that hurt readability |
| **Suggestion** | Style preferences, alternative approaches, nice-to-haves |
| **Question** | Reviewer is asking for clarification, not requesting a change |
| **Outdated** | The code has already changed and the comment no longer applies |

### 3.3 Group related comments

Comments that ask for the same kind of change across different files (e.g., "add error handling" in three places, or "rename this variable" in multiple spots) should be grouped. This lets the user address them as a batch instead of one by one.

Groups should have a short descriptive label (e.g., "Add null checks to API handlers", "Rename `data` to `userData` across modules").

### 3.4 Assess effort

For each comment or group, estimate the effort:
- **Trivial**: one-liner, rename, typo fix
- **Small**: a few lines, straightforward logic change
- **Medium**: requires touching multiple files or adding tests
- **Large**: architectural change, significant refactor

## Phase 4: Present the overview

Start with a quick summary so the user immediately knows what they're dealing with:

```
## PR #123: "Add user authentication"
**X unresolved comments** from @reviewer1, @reviewer2

### Breakdown
- N critical (need immediate attention)
- N should-fix
- N suggestions
- N questions
- N outdated (can likely be dismissed)
```

Then list each comment or group as a one-liner with its category and effort:

```
1. [Critical/Medium] **Missing auth check on /admin endpoint** — @reviewer1 (auth.ts:42)
2. [Should-fix/Small] **Add error handling to API calls** — @reviewer1 (3 locations)
3. [Suggestion/Trivial] **Use optional chaining** — @reviewer2 (utils.ts:15)
4. [Question] **"Why not use the existing UserService?"** — @reviewer2 (auth.ts:88)
5. [Outdated] **Variable naming in old helper** — @reviewer1 (helpers.ts:20)
```

### Presentation strategy

The amount of detail depends on the situation:

- **Critical comments exist**: After the overview, immediately dive into the critical ones, presenting them one by one with full context (the reviewer's comment, the code in question, and your assessment of what needs to change).
- **Few comments (1-3), none critical**: Present them all with full context at once — the user can see everything and decide.
- **Many comments (4+), none critical**: Show the overview list first, then ask the user how they'd like to proceed — all at once, by category, or one by one.

Always show the reviewer's exact words and the relevant code context (the diff hunk plus surrounding lines if needed to understand).

## Phase 5: User decides the course of action

For each comment or group, the user can:

1. **Fix it** — You'll make the code change, write/update tests, and commit
2. **Reply** — You'll help draft a reply (explaining a design decision, asking for clarification, or respectfully disagreeing)
3. **Skip** — Move on, come back to it later
4. **Dismiss as outdated** — Reply noting the code has changed and resolve the thread

Before starting any fixes, ask the user once:

> "Should I create **fixup commits** (I'll auto-rebase at the end) or **individual commits** for each fix?"

Remember their choice for the rest of the session.

## Phase 6: Fix

When the user chooses to fix a comment:

### 6.1 Understand the full context

Before changing anything, make sure you understand:
- What the reviewer is asking for
- The surrounding code and how the change fits in
- Whether existing tests cover this area
- What the right fix is (not just the minimal change)

### 6.2 Make the fix

Apply the code change. This is real production code — treat it with the same care as any other change:

- **Write or update tests.** If the fix changes behavior, there must be a test that covers it. If the reviewer pointed out a bug, write a test that would have caught it. Check for existing test files for the module and add to them.
- **Follow existing patterns.** Match the code style, error handling approach, and architecture of the surrounding code.
- **Don't over-fix.** Address what the reviewer asked for. Don't refactor the neighborhood.

### 6.3 Commit

Based on the user's earlier choice:

**Fixup commits:**
```bash
git add <files>
git commit --fixup=<original-commit-sha>
```

Find the right original commit by checking which commit last touched the lines in question:
```bash
git log --format="%H %s" "$BASE_REF"..HEAD -- <file>
```

Pick the commit that introduced the code the reviewer commented on.

**Individual commits:**
```bash
git add <files>
git commit -m "fix: <description of what was fixed and why>"
```

Use conventional commit format. The message should make sense on its own — someone reading `git log` should understand what changed and why.

### 6.4 Verify

After committing, run relevant tests to make sure the fix works and nothing is broken:

```bash
# Run the specific test file(s) touched or related to the change
# The exact command depends on the project — check package.json, Makefile, etc.
```

If tests fail, fix them before moving on.

## Phase 7: Reply

After fixing a comment (or when the user chooses to reply without fixing):

### 7.1 Reply to the review thread

For fixes, the reply should briefly explain:
- **What** was changed
- **Why** (the reasoning, not just "per your comment")
- **How** (if the approach isn't obvious from the diff)
- **Commit reference** — include the short SHA so the reviewer can jump straight to it

Keep it concise — reviewers appreciate brevity. Get the commit SHA with:
```bash
git rev-parse --short HEAD
```

Examples of good replies:

> "Fixed — the test now asserts `code == 0` and verifies the binary was updated with expected content. (commit c244245)"

> "Fixed — now uses `effectiveServer` for the `IsLocalServer` check. Also extended `IsLocalServer` to recognize `127.0.0.1` in addition to `localhost`. Added test case for the edge case. (commit a1b2c3d)"

> "Fixed — replaced raw `Read` with a `bufio.Scanner` that reads exactly one line per call. Added `TestTerminalPrompter_ReadsExactlyOneLine` which verifies two consecutive prompts from a multi-line buffer. (commit d4e5f6a)"

Notice the pattern: "Fixed — [what changed in plain language]. [tests added if any]. (commit [sha])"

For non-fix replies (explanations, disagreements, questions), help the user draft something clear and constructive. Present the draft to the user for approval before posting.

### 7.2 Post the reply

```bash
gh api repos/{owner}/{repo}/pulls/{number}/comments \
  --method POST \
  --input - <<EOF
{
  "body": "<reply text>",
  "in_reply_to": <comment_database_id>
}
EOF
```

Use `in_reply_to` with the comment's `databaseId` (from the GraphQL response) to thread the reply under the original comment.

When addressing a **group** of related comments with a single fix, reply to each thread individually. The reply can reference the same commit but should be specific to what that particular thread raised — don't copy-paste identical replies.

### 7.3 Resolve the thread

After fixing and replying, resolve the thread so the reviewer knows it's been addressed:

```bash
gh api graphql -f query='
  mutation($threadId: ID!) {
    resolveReviewThread(input: {threadId: $threadId}) {
      thread { isResolved }
    }
  }
' -f threadId="<thread_node_id>"
```

Only resolve after the fix is committed and the reply is posted. If the user just replied without fixing (e.g., a clarification or disagreement), do NOT resolve — let the reviewer decide.

## Phase 8: Wrap up

After all comments have been addressed:

### 8.1 Auto-rebase (if fixup commits)

If the user chose fixup commits:

```bash
GIT_SEQUENCE_EDITOR=true git rebase --interactive --autosquash "$BASE_REF"
```

If the rebase has conflicts, stop and help the user resolve them.

### 8.2 Summary

Present a final summary:

```
## Done

- N comments fixed (N fixup commits, rebased)
- N replies sent
- N threads resolved
- N skipped

Remaining: N unresolved comments (skipped)
```

### 8.3 Push prompt

If there were fixes:

> "Ready to push? (`git push --force-with-lease` since we rebased)"

Wait for the user to confirm before pushing. Never force-push without explicit approval.
