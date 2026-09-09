# `lns-push`

Publishes an lns document from a workflow: validate, dry run into the job
summary, then push it under every tag given.

```yaml
- uses: lensapp/lens-sandbox/actions/setup-lns@lns-actions-v0
- uses: lensapp/lens-sandbox/actions/lns-login@lns-actions-v0
  with:
    username: ${{ vars.LNS_HUB_USERNAME }}
    password: ${{ secrets.LNS_HUB_TOKEN }}
- id: push
  uses: lensapp/lens-sandbox/actions/lns-push@lns-actions-v0
  with:
    file: mixins/gh/gh.yaml
    kind: mixin
    tags: |
      acme/gh:sha-${{ github.sha }}
      acme/gh:latest
    push: ${{ github.event_name != 'pull_request' }}
- run: echo "published ${{ steps.push.outputs.digest }}"
```

## Inputs

| Input | Default | Description |
| --- | --- | --- |
| `file` | — | Document to publish. Its directory roots the document's relative filesets and supplies the `README.md` layer. Required. |
| `tags` | — | References to publish, one per line (commas also separate). Required. |
| `push` | `true` | `false` stops after the dry run — what a pull request wants. |
| `kind` | _(empty)_ | `sandbox` or `mixin`, the kind the document must be. Empty accepts either. |
| `require-exact-tool-versions` | `true` | Fail when the dry run reports tool versions that resolve at push time. |

## Outputs

| Output | Description |
| --- | --- |
| `digest` | The manifest digest published, `sha256:…`. Empty when `push` is `false`. |
| `refs` | Every reference pushed, one per line. Empty when `push` is `false`. |
| `first-push` | `true` when the first tag's repository did not exist before this push. Empty when `push` is `false`. |

## Tags name a namespace

A tag is `<namespace>/<name>:<tag>` or `<host>/<namespace>/<name>:<tag>`.
`gh:latest` is refused: nothing here assumes a namespace, because the
namespace is yours, not this repository's. A tag without a host resolves
against `hub.lns.run`.

## What the job summary carries

The dry run's output verbatim — the filesets and README layer it packed, the
digest it would publish, and any mixin it would publish alongside — then, for
a real push, the digest and every reference pushed. When the repository did
not exist before, one more line:

> `acme/gh` is new and private. Publish it at <https://hub.lns.run/acme/gh/settings>

The hub decides that a new repository is private. Making it public is a
setting on the hub until an API exists for it
([#398](https://github.com/lensapp/lens-sandbox/issues/398), work item 7).

`first-push` comes from an anonymous `GET /v2/<repo>/tags/list` before the
push: a `404` means new. A repository that exists but is private answers `401`
and is correctly reported as not new.

## Requirements

`lns` on `PATH` (run [`setup-lns`](../setup-lns/) first) and, for a real push,
a credential from [`lns-login`](../lns-login/).

The documents in [`fixtures/`](fixtures/) are what this repository's own CI
pushes to exercise the action.
