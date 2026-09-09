# `lns-logout`

Removes one registry's stored credential. It calls `lns logout`; when the
runner has no `lns-service` it deletes that one entry from
`~/.lns/registry-auth.json` itself, leaving every other host in the file alone.

```yaml
- uses: lensapp/lens-sandbox/actions/lns-logout@lns-actions-v0
  if: always()
  with:
    registry: hub.lns.run
```

## Inputs

| Input | Default | Description |
| --- | --- | --- |
| `registry` | `hub.lns.run` | Registry host, a bare `host[:port]` — no scheme, no path. |

A registry that is not logged in is not an error: the step is safe under
`if: always()`.

## Requirements

`lns` on `PATH` (run [`setup-lns`](../setup-lns/) first) and `jq`, which both
GitHub-hosted runner images carry.
