# Credentials

Tools inside the sandbox often need something that *looks* like a credential — a
`GITHUB_TOKEN`, an `OPENAI_API_KEY` — to behave normally. Lens Sandbox gives the
workload a credential-shaped **placeholder** instead of the real secret. The real
value stays outside the workload and is swapped in at the network boundary, only
for the right destination, and only when your policy allows it.

## How placeholders work

1. The workload starts with a placeholder seeded into the provider's environment
   variable — e.g. `GITHUB_TOKEN=ghp_LNSPLACEHOLDER...`. The placeholder is
   syntactically valid (tools accept it) but self-identifying as fake.
2. The workload makes a request to the provider's domain carrying that placeholder
   (in a header or a URL, depending on the provider).
3. At the boundary, the background service recognizes the placeholder and injects
   the **real** value in its place — but only for the configured domain, and only
   if your value decision allows it.

The workload behaves as though it holds the credential without the secret ever
entering the sandbox. A request carrying a placeholder to a domain the provider
isn't configured for is not rewritten — the placeholder goes nowhere useful.

## Built-in providers

These providers are recognized out of the box:

| Provider    | Environment variable  | Injected for          |
| ----------- | --------------------- | --------------------- |
| `github`    | `GITHUB_TOKEN`        | `api.github.com`, `github.com` |
| `openai`    | `OPENAI_API_KEY`      | `api.openai.com`      |
| `anthropic` | `ANTHROPIC_API_KEY`   | `api.anthropic.com`   |
| `linear`    | `LINEAR_API_KEY`      | `api.linear.app`      |
| `telegram`  | `TELEGRAM_BOT_TOKEN`  | `api.telegram.org`    |

## Value decisions

A provider's *value decision* is per-machine — it's how the real secret (or the
choice to deny) is bound on your machine. It's stored in `~/.lns-credentials.json`,
separate from the shareable `lns-policy.yaml`, so secrets are never committed. Set
it with `lns credential set`, which requires exactly one of:

```bash
# Use the value Lens Sandbox detects on the host for this provider
lns credential set github --host

# Store a specific real value at the boundary. Prefer --value-stdin so the
# secret stays out of your shell history and the process list.
printf '%s' "$MY_TOKEN" | lns credential set openai --value-stdin
lns credential set openai --value sk-...            # also works, less safe

# Deny: requests carrying this placeholder fail at the boundary
lns credential set telegram --deny
```

If a provider has no value decision yet, the first request that carries its
placeholder pauses for an approval — the same allow / deny flow as
[network policy](policy.md), deciding whether to inject the real value.

Clear a decision so the next use prompts again:

```bash
lns credential clear github
```

List providers and their current decisions:

```bash
lns credential list
```

## Custom providers

Declare your own provider for an internal API. The declaration is written to the
policy file (`lns-policy.yaml` by default, or `--policy <path>`):

```bash
lns credential add acme \
  --env-var ACME_API_TOKEN \
  --inject bearer_header:api.acme.internal
```

- `id` — the new provider id; it must not collide with a built-in or an existing
  custom provider.
- `--env-var` — the environment variable the placeholder is seeded into.
- `--inject KIND:DOMAIN` — how and where the real value is injected (repeatable).
  `KIND` is `bearer_header`, `uri_placeholder`, `token_header`,
  `basic_x_access_token`, or `api_key_header` (see
  [Injection kinds](#injection-kinds) below). For `api_key_header`, append the
  header name as a third segment: `--inject api_key_header:DOMAIN:HEADER`
  (e.g. `api_key_header:api.anthropic.com:x-api-key`).
- `--placeholder` — a specific placeholder value; auto-generated when omitted.
- `--value` / `--value-stdin` — optionally store the real value at the boundary in
  the same step (prefer `--value-stdin`).

The declaration lands under `credentials.customProviders` in the policy file
(declarations are shareable — only the *value* stays in `~/.lns-credentials.json`):

```yaml
network:
  allowedRoutes: []
  defaultVerdict: ask
  defaultTransport: direct
credentials:
  customProviders:
    - id: acme
      envVar: ACME_API_TOKEN
      placeholder: acme_LNSPLACEHOLDER0000000000000000000000
      injections:
        - kind: bearer_header
          domain: api.acme.internal
```

Add another domain injection to an existing custom provider:

```bash
lns credential add-injection acme --inject bearer_header:api2.acme.internal
```

Remove a custom provider (built-ins cannot be removed):

```bash
lns credential remove acme
```

### Injection kinds

An injection's `kind` decides how the real value reaches the destination:

| Kind                   | Effect on the request                                                 |
| ---------------------- | --------------------------------------------------------------------- |
| `bearer_header`        | `Authorization: Bearer <value>`                                       |
| `uri_placeholder`      | Value substituted into the request URI                                |
| `token_header`         | `Authorization: token <value>`                                        |
| `basic_x_access_token` | HTTP Basic auth as `x-access-token:<value>`                           |
| `api_key_header`       | A named header (the injection's `header:`, e.g. `x-api-key: <value>`) |

`lns credential add` and `add-injection` accept all five non-AWS kinds.
`api_key_header` requires the header name as a third segment
(`--inject api_key_header:DOMAIN:HEADER`); the other four are headerless and use
the two-segment form (`--inject KIND:DOMAIN`). `awsSigv4` is unsupported as a
static placeholder — it carries real STS material.

## See also

- [Policy and approvals](policy.md) — credential decisions follow the same
  allow / deny / ask model as network rules.
- [CLI reference](cli-reference.md) — the full `lns credential` flag list.
