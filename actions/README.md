# lns GitHub Actions

Composite actions that run the lns CLI in a workflow. They are released
together as one release-please component, `lns-actions`, tagged
`lns-actions-vX.Y.Z`.

| Action | What it does |
| --- | --- |
| [`setup-lns`](setup-lns/) | Installs the lns CLI and puts it on `PATH`. |
| [`lns-login`](lns-login/) | Stores a hub credential on the runner. |
| [`lns-logout`](lns-logout/) | Removes one registry's stored credential. |
| [`lns-push`](lns-push/) | Validates, dry-runs and publishes a document. |

## Usage

```yaml
- uses: lensapp/lens-sandbox/actions/setup-lns@lns-actions-v0
  with:
    version: 0.25.0 # or omit: newest lns release
- uses: lensapp/lens-sandbox/actions/lns-login@lns-actions-v0
  with:
    username: ${{ vars.LNS_HUB_USERNAME }}
    password: ${{ secrets.LNS_HUB_TOKEN }}
- uses: lensapp/lens-sandbox/actions/lns-push@lns-actions-v0
  with:
    file: lns.yaml
    tags: acme/hermes:${{ github.ref_name }}
```

Tags name a namespace: `<namespace>/<name>:<tag>` or
`<host>/<namespace>/<name>:<tag>`. Nothing here assumes one.

## Pin policy

Reference the actions by the floating major tag — `lns-actions-v0` before 1.0,
`lns-actions-v1` after. The release workflow force-moves that tag to every new
`lns-actions-vX.Y.Z`, so a workflow picks up fixes without an edit, and a major
bump is always an explicit change in your repository.

Pin the exact tag (`@lns-actions-v0.1.0`) or a commit SHA where a policy
demands immutable third-party references. A SHA pin never moves, so security
fixes need a Dependabot bump.

The lns CLI version is a separate decision: it is `setup-lns`'s `version`
input, not the action tag. An lns release does not open a pull request here.

## Conventions

The actions are composite — no Node build, no committed `dist/`. Shell that two
actions share lives in `actions/lib/`. Every `run:` block passes shellcheck,
and the `actions` CI job runs each action from the checkout on `ubuntu-latest`
and `macos-latest` against the newest lns release and a pinned one.
