# Runbook — Bumping the guest kernel

The lns guest kernel is pinned in [`crates/lns-service/kernels.toml`](../crates/lns-service/kernels.toml).
This runbook walks through bumping it to a newer Kata Containers
release. Two human gates, two PRs, the rest is automation.

## TL;DR

```sh
cargo run -p bump-kernel -- --kata-version 3.31.0
```

…then review + merge the PR that opens. Wait for release-please to
propose an lns-cli release, review + merge that too. Done.

## When to bump

Kata Containers cuts ~4-6 releases per year. You typically skip most
of them. Reasons to bump:

- **Security advisory in Kata's release notes** affecting the kernel
  variant you ship (mainline by default).
- **Kernel feature you need** for upcoming lns-cli work — a new
  syscall, eBPF surface, mount option, etc.
- **Performance improvement** with measurable impact on lns workloads.
- **Long-time-since-bump hygiene** — at most quarterly to stay close
  to upstream LTS.

Skip if: the release notes don't mention anything that affects you,
or the bump introduces upstream churn you don't want to absorb.

## How to bump

### Step 1 — Open the bump PR

From a clean working tree on `main` (or any base branch you want to
target):

```sh
cargo run -p bump-kernel -- --kata-version <kata-tag>
```

Flags:

| Flag | Default | Use when |
|---|---|---|
| `--kata-version <tag>` | required | the Kata release tag (e.g. `3.31.0`) |
| `--variant <variant>` | `mainline` | shipping a non-default variant (see [Variant choice](#variant-choice)) |
| `--commit-type {feat,fix,chore}` | `feat` | `fix` for security-driven bumps, `chore` to batch with other work |
| `--dry-run` | off | preview the manifest change without touching git |
| `-y, --yes` | off | skip the interactive confirmation |

The CLI does this for you:

1. Validates the Kata release tag exists upstream (HEAD on
   `https://github.com/kata-containers/kata-containers/releases/tag/<tag>`).
2. Checks `gh` CLI is installed and authenticated.
3. Requires a clean git tree (commit or stash any local edits first).
4. Refuses if the target branch `chore/bump-kernel-<tag>` already
   exists locally or on origin.
5. Edits `crates/lns-service/kernels.toml`:
   - `kata_version` → your input
   - `kernel_variant` → your variant
   - `kernel_filename`, `published_version` → `"pending"`
   - all `[current.sha256]` values → `""`
6. Creates branch `chore/bump-kernel-<tag>`, commits with subject
   `<type>(lns): bump guest kernel to Kata <tag>`, pushes, opens a PR
   via `gh`.

### Step 2 — Wait for the bot back-fill (~3–5 min)

CI's `publish-kernel` workflow first runs `detect-bump` to confirm
this PR actually modifies `kernels.toml` (a `bump-kernel`-driven PR
always does; pure workflow-file edits, deletions, or the
introduction PR are short-circuited so they don't trip the
overwrite-refusal guard). Then the `compute` job runs per-arch:

1. Validates the Kata tarball URL is reachable.
2. Refuses if the CDN origin already has an artifact at the destination URL
   (the [overwrite refusal](#emergency-overwriting-an-already-published-artifact)
   contract).
3. Downloads Kata's `kata-static-<tag>-<arch>.tar.zst`.
4. Resolves the kernel filename for the requested variant.
5. Extracts, conditionally gunzips (aarch64 only), computes sha256.

Then the `back-fill` job aggregates the per-arch results and **pushes
a commit on your PR branch** named
`chore(kernel): fill provenance and shas for <published-version>`.
That commit writes the resolved `kernel_filename`, `published_version`,
and per-arch shas into the manifest.

### Step 3 — Re-run CI on the back-filled commit

GitHub Actions ships with a deliberate anti-recursion safety: commits
pushed by the workflow's default `GITHUB_TOKEN` **do not retrigger**
workflows. So the bot's back-fill commit lands but CI doesn't run on
it automatically.

> **You click**: PR → **Checks** tab → **Re-run all jobs**.

After ~40 seconds, CI runs against the back-filled state and goes
green:

- `build.rs` parses the now-complete `kernels.toml`, emits the new
  `KERNEL_VERSION` / `KERNEL_SHA256` constants.
- `cargo test` validates the format checks in `kernel.rs`'s tests.
- The drift CI step runs in `--allow-pending` mode (your PR is
  modifying `kernels.toml`, so CDN 404s are tolerated until merge).

### Step 4 — Review the PR

What to look for:

| Field | What to check |
|---|---|
| `kata_version` | Matches the Kata release tag you intended. |
| `kernel_variant` | The variant you wanted (mainline unless you have a reason). |
| `kernel_filename` | The bot wrote `vmlinuz-<x.y.z-build>`. Cross-check against the workflow run's job summary. |
| `published_version` | Equals `kernel_filename` minus the `vmlinuz-` prefix. |
| `[current.sha256]` | Two values, both 64 hex chars. Cross-check against the workflow run's matrix outputs. |
| Bot commit author | `github-actions[bot]` (the GITHUB_TOKEN-authored back-fill). |
| Commit subject | Your original `<type>(lns): bump guest kernel to Kata <tag>`. |

If everything looks right, **merge**.

### Step 5 — Merge triggers the actual CDN upload

On push to `main`, `publish-kernel`'s `publish` job fires (per-arch
matrix):

1. Defence-in-depth overwrite check against the CDN origin (should always
   pass at this point).
2. Re-downloads Kata, recomputes sha256 (deterministic for fixed
   inputs).
3. Asserts the recomputed sha matches the merged manifest.
4. Uploads bytes + `<artifact>.sha256` sidecar to the CDN origin under
   `s3://${S3_BUCKET}/lns-kernel-<version>-<arch>`.

End users can now `curl https://get.lns.run/lns-kernel-<version>-<arch>`.

### Step 6 — release-please proposes an lns-cli release PR

The `feat(lns): bump guest kernel...` commit on `main` is picked up
by release-please's `crates/lns-cli` tracking. A release PR opens
titled `chore(lns): release <next-version>` with the CHANGELOG entry.

**You merge that PR.** The tag `lns-v<next-version>` is cut, and
`build-lns` builds binaries for darwin-aarch64 / linux-aarch64 /
linux-x86_64 with the new `KERNEL_VERSION` + `KERNEL_SHA256`
compiled in via `build.rs`. The binaries hit GitHub Release and the
`get.lns.run` CDN; the install script picks up the new
`lns-latest.json`.

## Variant choice

`kernel_variant` selects which `vmlinuz-*` file inside Kata's tarball
to publish. Kata ships several:

| Variant | Filename pattern | When |
|---|---|---|
| `mainline` | `vmlinuz-X.Y.Z-N` (no suffix) | **Default.** What you almost always want. |
| `confidential` | `vmlinuz-X.Y.Z-N-confidential` | TEE / SEV-SNP / TDX use cases. Not currently used by lns. |
| `tdx` / `sev` / `snp` | `vmlinuz-X.Y.Z-N-<variant>` | Vendor-specific confidential-computing variants. |
| `dragonball` | `vmlinuz-X.Y.Z-N-dragonball-experimental` | Kata's experimental Dragonball hypervisor. Not for production. |

If you don't have a specific reason, leave `--variant` at `mainline`.

## Rollback

If a published kernel breaks something post-release:

1. **`git revert` the kernel-bump PR.** The bumped `kernels.toml`
   reverts to the previous pin. release-please picks up the revert
   as another commit and proposes a new release PR.
2. **Merge the release PR.** A new lns-cli version ships with the
   restored pin.
3. **The old CDN URL still exists.** Artifacts at
   `lns-kernel-<old-version>-<arch>` are never deleted (immutable per
   release), so the reverted code path resolves cleanly.

There's no need to manually delete the new (problematic) artifact.
It just stops being referenced. End users see the rolled-back
version on the next install / update.

## Emergency: overwriting an already-published artifact

This is **almost never the right answer.** Kernel artifacts at
`lns-kernel-<version>-<arch>` are designed to be **immutable per
release** — the CDN pull zone caches them for up to 1 year, so
overwriting in place causes CDN-edge stale-cache divergence that
takes effort to fully purge.

**Preferred recovery: pick a new `published_version`.** Bump the
filename's `-N` build suffix (e.g., `vmlinuz-6.18.15-192` →
`vmlinuz-6.18.15-193` if Kata has one available; otherwise file an
issue + manually rename the artifact name in `kernels.toml` to
something distinguishable like `6.18.15-192-r2`).

**If you absolutely must overwrite:**

Pull the destination + credentials from the repo Settings → Secrets
and variables → Actions:

- `S3_ENDPOINT_URL`  → set as `--endpoint-url` and `$ENDPOINT` below
- `S3_BUCKET`        → set as `$BUCKET` below
- `S3_ACCESS_KEY_ID` / `S3_SECRET_ACCESS_KEY` → export as
  `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`

Then:

```sh
# 1. Manually delete the existing object.
aws s3 rm "s3://${BUCKET}/lns-kernel-<version>-<arch>" \
    --endpoint-url "$ENDPOINT"

aws s3 rm "s3://${BUCKET}/lns-kernel-<version>-<arch>.sha256" \
    --endpoint-url "$ENDPOINT"

# 2. Purge the CDN pull-zone cache for both URLs via the provider's
#    dashboard.

# 3. Re-run the `publish-kernel` workflow on the merge commit via gh.
#    This uses the workflow's `workflow_dispatch` trigger, which
#    bypasses the bump-detection gate (operator decision).
gh workflow run publish-kernel.yml -r <commit-sha>
```

Only the `publish` job runs on dispatch — `compute` and `back-fill`
are PR-time only, so they skip.

The workflow's preflight will now find no existing object and
proceed. Users may still hit stale-cached old bytes from CDN PoPs
that haven't refreshed; the Rust-side sha pin in `kernel.rs` rejects
those (`bail!` at the sha-check; client sees a "Refusing to install"
error) until the PoP re-warms.

## Common errors

### CLI: `Kata release X.Y.Z not found upstream`

The Kata version you typed doesn't exist. Check
<https://github.com/kata-containers/kata-containers/releases>
for the exact tag (no `v` prefix; the format is plain `X.Y.Z`).

### CLI: `git working tree is not clean`

You have uncommitted changes. Commit, stash, or discard them — the
CLI refuses to clobber local work.

### CLI: `branch chore/bump-kernel-X.Y.Z already exists`

A bump for this version is already in progress (or stale). If a real
PR is open: contribute to that one. If it's stale:

```sh
git branch -D chore/bump-kernel-X.Y.Z          # local
git push origin --delete chore/bump-kernel-X.Y.Z   # remote
```

…then rerun the CLI.

### CLI: `the gh CLI is required`

Install: `brew install gh` (macOS) / `apt install gh` (Debian).
Then: `gh auth login`.

### CI: bump PR's first run is red, no bot commit yet

The compute job is still running. Kata's tarball is ~670 MB; the
download + extract + sha takes 3-5 min per arch. Both arches run in
parallel. Wait, then click "Re-run all jobs" after the bot commit
appears.

### CI: bump PR's first run is red, bot commit appeared, still red

You forgot to click "Re-run all jobs." The bot commit doesn't
auto-retrigger CI (see [Step 3](#step-3--re-run-ci-on-the-back-filled-commit)).

### CI: `published_version=... doesn't match strip_prefix(kernel_filename, "vmlinuz-")`

Manifest is internally inconsistent. Either the CLI was overridden by
manual edits, or someone edited the manifest after the bot back-filled.
Run `bump-kernel` again — it'll reset to the consistent state.

### CI: `s3://.../lns-kernel-X.Y.Z-<arch> already exists in storage`

The published_version you're proposing has already been published.
This is the overwrite-refusal contract. Either pick a different
`published_version` (rare — usually it means you accidentally tried
to re-publish the same Kata version + filename combination), or
follow the emergency-overwrite procedure above (rare).

### CI: `sha mismatch — refusing to upload`

The Kata bytes the publish job downloaded don't hash to what the
manifest says. Possible causes:

- Upstream Kata yanked the release and re-uploaded different bytes
  (very rare; their tags are usually immutable).
- The manifest was tampered between PR merge and publish — someone
  edited `[current.sha256]` directly on `main` without going through
  the workflow.

Investigate via `git log -p crates/lns-service/kernels.toml` and run
`bump-kernel` again to reset.

## References

- Manifest: `crates/lns-service/kernels.toml`
- Build-time derivation: `crates/lns-cli/build.rs`
- Runtime download + verify: `crates/lns-cli/src/kernel.rs`
- Operator CLI + CI helpers: `crates/bump-kernel/` — subcommands
  `bump` (operator), `show` (CI manifest reader), `back-fill` (CI
  per-arch aggregator)
- Publish workflow: `.github/workflows/publish-kernel.yml`
- Drift check (called from `ci.yml`): `scripts/check-kernel-pins.sh`
