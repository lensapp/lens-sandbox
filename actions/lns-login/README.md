# `lns-login`

Stores a hub credential on the runner so a later `lns artifact push` or
`lns run` can authenticate. It calls `lns login --password-stdin`; when the
runner has no `lns-service` — the usual case, since `setup-lns` installs the
CLI only — it writes the one entry into `~/.lns/registry-auth.json` itself,
mode 0600, leaving every other host in the file alone.

```yaml
- uses: lensapp/lens-sandbox/actions/setup-lns@lns-actions-v0
- uses: lensapp/lens-sandbox/actions/lns-login@lns-actions-v0
  with:
    registry: hub.lns.run
    username: ${{ vars.LNS_HUB_USERNAME }}
    password: ${{ secrets.LNS_HUB_TOKEN }}
```

## Inputs

| Input | Default | Description |
| --- | --- | --- |
| `registry` | `hub.lns.run` | Registry host, a bare `host[:port]` — no scheme, no path. |
| `username` | — | Username the token belongs to. Required. |
| `password` | — | Password or token. Required. |

## The token

Pass a secret, never a literal. The action masks it with `::add-mask::` before
anything else runs, and hands it to the script through the environment — it is
never an argument to any process, so it cannot appear in `ps` output. `%`,
carriage returns and newlines are escaped before the mask is emitted, so a
multi-line token is masked as entered rather than spilling its later lines into
the log.

Log out at the end of a job that runs untrusted steps afterwards:

```yaml
- uses: lensapp/lens-sandbox/actions/lns-logout@lns-actions-v0
  if: always()
  with:
    registry: hub.lns.run
```

## Requirements

`lns` on `PATH` (run [`setup-lns`](../setup-lns/) first) and `jq`, which both
GitHub-hosted runner images carry.
