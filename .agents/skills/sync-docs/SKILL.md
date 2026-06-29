---
name: sync-docs
description: >
  Sync the `docs/` subtree with the authoritative `lens-product` repo, in either direction.
  Use when the user asks to:
  (1) pull / update / refresh docs from lens-product (default direction),
  (2) push / send / sync local docs/ edits back to lens-product as a PR there,
  (3) "sync docs", "sync the subtree", or any wording about reconciling `docs/` with upstream.
  Knows how to open the PR in the correct repository for whichever direction is requested.
model: sonnet
---

# Syncing `docs/` with `lens-product`

The `docs/` directory at the repo root is a `git subtree` of
[`lensapp/lens-product/products/lens-sandbox`](https://github.com/lensapp/lens-product/tree/main/products/lens-sandbox).

**`lens-product` is authoritative.** Edits should normally happen there, not here.
This skill exists so the two repos stay in sync when work does happen on either side.

> **Scope invariant (read this).** Only the `products/lens-sandbox/` subtree of
> `lens-product` maps to `docs/` here. Never pull, push, or diff against
> `lens-product`'s root tree (`main`, `origin/main`, etc.) — that would drag in
> unrelated product docs (`_legacy/`, `products/lens-agents/`, `strategy/`,
> `memos/`, …). Every `subtree` command below operates on the split branch
> (`lens-sandbox-only`), never on `main` directly. If you catch yourself typing
> `git subtree pull --prefix=docs lens-product main` — stop.

## Modes

| Mode | Direction | When to use | PR is opened in |
|------|-----------|-------------|-----------------|
| **pull** (default) | `lens-product` → this repo | Upstream has new product docs and we want them visible here | `lensapp/lens-sandbox` |
| **push** (rare) | this repo → `lens-product` | Someone edited `docs/` here and the change must land upstream | `lensapp/lens-product` |

If the user just says "sync docs" with no direction, assume **pull**.

## Common setup (both modes)

`git subtree split` needs full history, so a shallow clone of `lens-product` will not work.

```bash
LENS_PRODUCT=/tmp/lens-product-src
rm -rf "$LENS_PRODUCT"
git clone https://github.com/lensapp/lens-product.git "$LENS_PRODUCT"
```

Also note the location of this repo so the push flow can `rsync` between them:

```bash
LENS_SANDBOX=$(git rev-parse --show-toplevel)
```

`git subtree pull` and `git subtree push` both create merge commits, which
require a committer identity. Fresh clones in ephemeral sandboxes have none
configured, and the failure mode is confusing (the subtree operation may appear
to succeed with no changes). Set identity in **both** clones before proceeding:

```bash
for repo in "$LENS_SANDBOX" "$LENS_PRODUCT"; do
  git -C "$repo" config user.email "lens-sandbox-dev@lensapp.com"
  git -C "$repo" config user.name  "Lens Sandbox Dev (agent)"
done
```

---

## Mode A — Pull (`lens-product` → this repo)

### 1. Split the upstream subdirectory into its own branch

Scope: `products/lens-sandbox/` only. Not the whole repo.

```bash
(cd "$LENS_PRODUCT" && git subtree split --prefix=products/lens-sandbox -b lens-sandbox-only)
UPSTREAM_SPLIT=$(cd "$LENS_PRODUCT" && git rev-parse lens-sandbox-only)
```

`$UPSTREAM_SPLIT` is the authoritative "what `docs/` should look like" SHA for
this run. Keep it in scope — steps 2 and 5 both reference it.

### 2. Preflight — detect no-op before touching anything

Compare the upstream split HEAD to the last split SHA recorded in `docs/`:

```bash
LAST_SPLIT=$(git log --grep="Squashed 'docs/'" -1 --format=%B \
  | sed -n 's/^git-subtree-split: //p')

if [ "$UPSTREAM_SPLIT" = "$LAST_SPLIT" ]; then
  echo "Already in sync ($UPSTREAM_SPLIT). Nothing to do."
  exit 0
fi
```

If the SHAs match, stop here — no branch, no pull, no PR. Tell the user
"already up to date" and exit. Only proceed when they differ.

### 3. Make a fresh branch off `origin/main` in this repo

```bash
git fetch origin main
git checkout -b chore/sync-docs-$(date +%Y%m%d) origin/main
```

### 4. Pull the split branch into `docs/`

The remote ref MUST be `lens-sandbox-only` (the split branch from step 1),
NEVER `main`. Pulling `main` would drop the entire lens-product tree into
`docs/` and violate the scope invariant.

```bash
git remote add lens-product "$LENS_PRODUCT" 2>/dev/null || true
git fetch lens-product lens-sandbox-only
git subtree pull --prefix=docs lens-product lens-sandbox-only --squash
git remote remove lens-product
```

If `git subtree pull` says **"Already up to date"**, abort — there is nothing
to sync. Tell the user and stop; do not open an empty PR. (The preflight in
step 2 should have caught this already; if you reach here and still see it,
something is off — investigate rather than proceeding.)

### 5. Verify the scope — fail loud on mismatch

Before pushing, confirm the squash commit you just created references the
expected split SHA. This is the guardrail that catches the "pulled from `main`
by mistake" failure mode.

```bash
NEW_SPLIT=$(git log -1 --grep="Squashed 'docs/'" --format=%B \
  | sed -n 's/^git-subtree-split: //p')

if [ "$NEW_SPLIT" != "$UPSTREAM_SPLIT" ]; then
  echo "SCOPE VIOLATION: squash commit references $NEW_SPLIT, expected $UPSTREAM_SPLIT"
  echo "The pull likely targeted the wrong ref (main instead of lens-sandbox-only)."
  echo "Reset the branch and re-run from step 4. Do NOT push."
  exit 1
fi
```

Also sanity-check the changed paths — every `docs/` path should look like a
lens-sandbox product doc, not something from `_legacy/`, `products/lens-agents/`,
`strategy/`, `memos/`, `foundation/`, etc.:

```bash
git diff --name-only origin/main..HEAD -- docs/ | head -50
```

If either check fails, reset the branch (`git reset --hard origin/main`) and
investigate — do not push a bad-scope sync.

### 6. Push and open the PR (in `lensapp/lens-sandbox`)

```bash
git push -u origin HEAD
```

PR title: `docs: Sync docs/ from lens-product`

PR body should include the list of upstream commits that are coming in:

```bash
(cd "$LENS_PRODUCT" && git log --oneline "$LAST_SPLIT".."$UPSTREAM_SPLIT" -- .)
```

`$LAST_SPLIT` is from step 2 (the previous `git-subtree-split:` trailer in
`docs/`). `$UPSTREAM_SPLIT` is from step 1.

---

## Mode B — Push (this repo → `lens-product`)

This path is the exception, not the default. Before running it, **ask the user**:

> "`lens-product` is the authoritative copy of these docs. Would it be cleaner to make this edit directly in `lens-product` instead of round-tripping through here?"

Proceed only if the user confirms the round-trip is intentional (e.g. the edit was discovered while working in this repo and reverting it just to redo it upstream isn't worth the friction).

### 1. Identify what changed in `docs/` locally

```bash
# Commits in this repo that touched docs/ since the last subtree merge
git log --oneline origin/main..HEAD -- docs/
# Or, against the last upstream sync point:
git log --oneline $(git log --grep="Squashed 'docs/'" -1 --format=%H)..HEAD -- docs/
```

Confirm with the user which commits / file changes should go upstream.

### 2. Stage the changes in a `lens-product` working copy

```bash
cd "$LENS_PRODUCT"
git fetch origin main
git checkout -b sync-from-lens-sandbox-$(date +%Y%m%d) origin/main

# Mirror docs/ from lens-sandbox into products/lens-sandbox/.
# Use rsync with --delete so file removals propagate. Never cp -r — it leaves
# orphaned files in the destination.
rsync -a --delete "$LENS_SANDBOX/docs/" products/lens-sandbox/
```

### 3. Commit in `lens-product`

```bash
cd "$LENS_PRODUCT"
git add products/lens-sandbox
git status   # confirm only products/lens-sandbox/ paths changed
git commit -m "docs(lens-sandbox): Sync changes from lensapp/lens-sandbox

Mirrors edits made in lensapp/lens-sandbox <PR-URL-or-commit-sha>.
"
```

Use a conventional commit message. Reference the originating PR or commit in this repo so reviewers upstream can trace context.

### 4. Push and open the PR (in `lensapp/lens-product`)

```bash
git push -u origin HEAD
gh pr create \
  --repo lensapp/lens-product \
  --title "docs(lens-sandbox): Sync changes from lens-sandbox" \
  --body "..."
```

PR body should link back to the source PR or commits in `lensapp/lens-sandbox` and summarise what changed.

### 5. After the upstream PR merges, re-run Mode A

Once `lens-product` has accepted the change, run **Mode A (pull)** in this repo. This is what re-aligns the `git subtree` breadcrumbs so the next pull works cleanly. Without this step the next pull will see the change as "new" upstream and try to merge it into already-modified files.

---

## Things to avoid

- **Don't** `git subtree pull` (or push) against `lens-product`'s `main` directly. Only `lens-sandbox-only` (the split branch from Mode A step 1). The scope-verification check in Mode A step 5 exists specifically to catch this mistake.
- **Don't** drop `--squash`. The initial `subtree add` was squashed; mixing squashed and non-squashed pulls corrupts the breadcrumb commits and breaks future `subtree pull`.
- **Don't** rewrite, squash, or amend the `Squashed 'docs/' content from commit <sha>` and `Merge commit '<sha>' as 'docs'` commits. `git subtree` uses them to find the merge base on the next pull.
- **Don't** edit files in `docs/` here without planning to push the change upstream. Drift quietly erodes the "lens-product is authoritative" guarantee.
- **Don't** use `git clone --depth=1` for `lens-product` — `git subtree split` needs full history and will fail silently or produce a wrong split.
- **Don't** force-push either PR. Both repos likely have branch protection.
- **Don't** open an empty pull-sync PR — if `subtree pull` reports "Already up to date", just tell the user and stop.
- **Don't** skip the git identity setup on fresh clones. `subtree pull` creates a merge commit and may fail in confusing ways (sometimes appearing to succeed with no changes) when no committer identity is configured.
