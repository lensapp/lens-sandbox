# Credentials

Tools inside the sandbox often need something that *looks* like a credential — an
`OPENAI_API_KEY`, an `ANTHROPIC_API_KEY` — to behave normally. Lens Sandbox gives the
workload a credential-shaped **placeholder** instead of the real secret. The real
value stays outside the workload and is swapped in at the network boundary, only
for the right destination, and only when your policy allows it.

## How placeholders work

1. The workload starts with a placeholder seeded into the provider's environment
   variable — e.g. `OPENAI_API_KEY=sk-LNSPLACEHOLDER...`. The placeholder is
   syntactically valid (tools accept it) but self-identifying as fake.
2. The workload makes a request to the provider's domain carrying that placeholder
   (in a header or a URL, depending on the provider).
3. At the boundary, the background service recognizes the placeholder and injects
   the **real** value in its place — but only for the configured domain, and only
   if your value decision allows it.

The workload behaves as though it holds the credential without the secret ever
entering the sandbox. A request carrying a placeholder to a domain the provider
isn't configured for is not rewritten — the placeholder goes nowhere useful.

## Providers are integrations

Every credential provider is an [integration](integrations.md): a named service that
bundles its placeholder, environment variable, and per-domain injection with the
routes it needs. `openai`, `anthropic`, `linear`, `telegram`, `gitlab`, and
`huggingface` ship in the bundled catalog; `github` ships as an `oauth` integration
(device sign-in). Declare your own for an internal API with `lns integration add`
(see [Integrations](integrations.md)). Connect one to a project with
`lns integration connect <id>`, which records it under `integrations:` in that
directory's `lns-policy.yaml`.

## Value decisions

A provider's *value decision* is per-machine — it's how the real secret (or the
choice to deny) is bound on your machine. It's stored in `~/.lns-credentials.json`,
separate from the shareable `lns-policy.yaml`, so secrets are never committed.

Decisions are made interactively: the first request that carries a placeholder
pauses for an approval — the same allow / deny / ask flow as
[network policy](policy.md) — where you choose to use the value Lens Sandbox detects
on the host, store a specific value at the boundary, or deny (requests carrying the
placeholder then fail at the boundary). The decision is remembered for next time.

An [`oauth` integration](integrations.md)'s value decision is different in kind: rather
than a pasted secret it's a self-renewing **token set** obtained by an interactive
device sign-in (`lns integration connect <id>`), refreshed automatically and
re-prompted when the grant can no longer be refreshed. It lives in the same per-machine
file and is never written to `lns-policy.yaml`.

## Injection kinds

An integration's injection `kind` decides how the real value reaches the destination:

| Kind                   | Effect on the request                                                 |
| ---------------------- | --------------------------------------------------------------------- |
| `bearer_header`        | `Authorization: Bearer <value>`                                       |
| `uri_placeholder`      | Value substituted into the request URI                                |
| `token_header`         | `Authorization: token <value>`                                        |
| `basic_x_access_token` | HTTP Basic auth as `x-access-token:<value>`                           |
| `api_key_header`       | A named header (the injection's `header:`, e.g. `x-api-key: <value>`) |

`lns integration add --inject` accepts all five non-AWS kinds. `api_key_header`
requires the header name as a third segment
(`--inject api_key_header:DOMAIN:HEADER`); the other four are headerless and use
the two-segment form (`--inject KIND:DOMAIN`). `awsSigv4` is unsupported as a
static placeholder — it carries real STS material.

## See also

- [Integrations](integrations.md) — declare, connect, and list the services whose
  credentials reach a workload.
- [Policy and approvals](policy.md) — value decisions follow the same
  allow / deny / ask model as network rules.
