# Claude Code in a sandbox

Runs [Claude Code](https://docs.claude.com/en/docs/claude-code) inside a Lens
Sandbox microVM. The agent runs unrestricted **inside** the guest; the microVM
and its network policy are the boundary — nothing leaves the sandbox except the
hosts you allow.

## Files

- **`lns.yaml`** — the sandbox definition: a slim Debian base with node declared
  under `spec.tools` (provisioned once per machine by the service, outside the
  policy cage), a first-boot install of Claude Code itself, the network
  allowlist, and inline seed state mounted at the workload's home (`/home/sandbox`). The inline `.claude.json` skips onboarding
  and pre-accepts the `/workspace` trust dialog. The inline
  `.claude/settings.json` runs Claude in `bypassPermissions` and turns **off
  Claude's own sandbox**. Both are redundant here: the lns microVM is already
  the sandbox, so the agent should run freely inside it.

## Use it

Copy the self-contained `lns.yaml` into the project you want the agent to work
on (`.` is bound at `/workspace`, so it operates on that directory), then run:

```bash
cd your-project      # now contains lns.yaml
lns run
```

node comes from `spec.tools`: the service provisions `node@22` once per machine
before the microVM boots — no base-image coupling, no per-run download, no
policy route. The first boot still runs `npm install -g
@anthropic-ai/claude-code` for the agent itself; the microVM is ephemeral, so
each cold start reinstalls that package (the `egress.http` rules keep
`registry.npmjs.org` open for exactly that install).

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

One artifact uploads: the sandbox definition, including its inline fileset. The
base image is referenced as written in `spec.image`, not re-uploaded — a
consumer pulls it from its origin (`docker.io`). `lns push`
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

The package starts **private**; make it public or grant access on the settings
page for someone else to pull. They will also need `docker.io` reach for the
base image.

## Notes

- The network allowlist is deliberately tight. `registry.npmjs.org` is only
  needed for the first-boot `npm install` of Claude Code itself — node already
  arrives via [`spec.tools`](../../running-workloads.md#tools--declared-toolchains),
  provisioned by the service outside the policy cage. Baking Claude Code into a
  custom image and pushing that remains an option that drops the route and the
  runtime install entirely.
- Everything except `/workspace` is ephemeral. Session history and any config
  Claude writes at runtime die with the microVM; the inline seed is restored
  from `lns.yaml` on every run. Attach a named volume if you want state to
  persist.
