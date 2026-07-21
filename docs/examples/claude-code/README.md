# Claude Code in a sandbox

Runs [Claude Code](https://docs.claude.com/en/docs/claude-code) inside a Lens
Sandbox microVM. The agent runs unrestricted **inside** the guest; the microVM
and its network policy are the boundary — nothing leaves the sandbox except the
hosts you allow.

## Files

- **`lns.yaml`** — the sandbox definition: a `node:lts-alpine` base that installs
  Claude Code on first boot, the network allowlist, and the
  `claude-code-subscription` integration.
- **`config/`** — seed state mounted at the workload's home (`/home/sandbox`):
  - `config/.claude.json` — skips onboarding and pre-accepts the `/workspace`
    trust dialog.
  - `config/.claude/settings.json` — runs Claude in `bypassPermissions` and turns
    **off Claude's own sandbox**. Both are redundant here: the lns microVM is
    already the sandbox, so the agent should run freely inside it.

## Use it

Copy `lns.yaml` and `config/` into the project you want the agent to work on
(`.` is bound at `/workspace`, so it operates on that directory), then run:

```bash
cd your-project      # now contains lns.yaml + config/
lns run
```

The first boot runs `npm install -g @anthropic-ai/claude-code`; the microVM is
ephemeral, so each cold start reinstalls it (the `allowedRoutes` keep
`registry.npmjs.org` open for that reason).

On first run, an approval card asks for your Claude subscription token and shows
how to mint it.

## Publish and run from a registry

This recipe is a normal sandbox artifact, so instead of copying the files around
you can publish it once and run it by reference from any directory. The example
below uses GitHub Container Registry (GHCR); see
[Distributing a sandbox](../../running-workloads.md#distributing-a-sandbox) for
the full mechanics. Each command derives your namespace from
`gh api user --jq .login`, so you can paste them as-is — the `tr A-Z a-z` on the
image paths keeps a capitalized username valid (GHCR paths must be lowercase).

Log in once — a GitHub token with `write:packages` (the `gh` CLI mints one):

```bash
gh auth refresh --scopes write:packages,read:packages
gh auth token | lns login ghcr.io --username "$(gh api user --jq .login)" --password-stdin
```

Publish this directory's `lns.yaml`:

```bash
lns push --dry-run "ghcr.io/$(gh api user --jq .login | tr A-Z a-z)/claude-code:0.1.0"   # preview digests, upload nothing
lns push "ghcr.io/$(gh api user --jq .login | tr A-Z a-z)/claude-code:0.1.0"             # for real
```

Two artifacts upload: the sandbox definition and the packed `./config` fileset
(digest-pinned). The base image is referenced as written in `spec.image`, not
re-uploaded — a consumer pulls it from its origin (`docker.io`). `lns push`
does not resolve the tag to a digest, so pin `spec.image` by digest yourself if
you need a byte-reproducible base.

Run it from anywhere. `.` binds to the *consumer's* directory, and the seed
config and policy baseline arrive inside the artifact:

```bash
cd ~/some-project
lns inspect "ghcr.io/$(gh api user --jq .login | tr A-Z a-z)/claude-code:0.1.0"   # review what shipped before running
lns run     "ghcr.io/$(gh api user --jq .login | tr A-Z a-z)/claude-code:0.1.0"   # `.` -> /workspace is ~/some-project
```

Edit and re-publish under a new tag so the update is unambiguous:

```bash
lns push "ghcr.io/$(gh api user --jq .login | tr A-Z a-z)/claude-code:0.2.0"
```

Reusing the same tag instead? Run `lns pull` on it first — a tag is mutable, so
pull re-consults the registry. For a byte-exact hand-off, run the digest the push
printed (`lns run ghcr.io/…@sha256:…`).

### View it on GitHub

After a real push, print the package links (paste-and-open):

```bash
echo "https://github.com/$(gh api user --jq .login)?tab=packages"                                    # all packages
echo "https://github.com/users/$(gh api user --jq .login)/packages/container/package/claude-code"    # this package
echo "https://github.com/users/$(gh api user --jq .login)/packages/container/claude-code/settings"   # visibility / access
```

The package lists your tags plus an untagged digest — that untagged one is the
`./config` fileset. It starts **private**; make it public or grant access on the
settings page for someone else to pull, and they will also need `docker.io` reach
for the base image.

## Notes

- The network allowlist is deliberately tight. `registry.npmjs.org` is only
  needed for the first-boot install; if you bake Claude Code into a custom image
  and `lns push` it, you can drop that route and the runtime install entirely.
- Everything except `/workspace` is ephemeral. Session history and any config
  Claude writes at runtime die with the microVM; the seed `config/` is one-way
  (host → guest). Attach a named volume if you want state to persist.
