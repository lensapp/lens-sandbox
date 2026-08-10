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

## Secrets that never cross HTTP

The boundary swap only fits a secret that travels in a header or a URI. A signing
key does not: `gpg` and `ssh` talk to a local agent over a unix socket, so there is
no request to rewrite. Those secrets stay outside the workload a second way —
[host access](running-workloads.md#host-access) forwards the *agent*, not the key,
so the workload can ask your agent to sign and never holds the private half.

## Providers are connectors

Every credential provider is a [connector](connectors.md): a named service that
bundles its placeholder, environment variable, and per-domain injection with the
routes it needs. `openai`, `anthropic`, `bedrock`, `linear`, `telegram`, `gitlab`,
and `huggingface` ship in the bundled catalog; `github` ships as an `oauth` connector
(device sign-in) and `openrouter` as an `oauth` connector (pkce browser sign-in).
Declare your own for an internal API with `lns connector add`
(see [Connectors](connectors.md)). A sandbox definition that lists a provider
under `spec.connectors` seeds its placeholder env var but only *offers* it —
the workload is prompted on first use, never armed automatically.
`lns connector connect <id>` records the id under `connectors:` in that
directory's `lns-policy.yaml` and binds the value on your machine, but that
alone does not arm the credential for a workload — see
[Workload grants](#workload-grants) below.

## Value decisions

A provider's *value decision* is per-machine — it's how the real secret is bound
on your machine. It's stored in `~/.lns-credentials.json`, separate from the
shareable `lns-policy.yaml`, so secrets are never committed.

Decisions are made interactively, at either of two moments:

- **Reactively** — the first request that carries a placeholder pauses for an
  approval, the same allow / deny / ask flow as [network policy](policy.md).
- **Proactively** — `lns connector connect <id>` raises the same card in the
  approval window before any run, which is how you bind a credential a sandbox
  definition **requires**: a required slot (`spec.credentials` with
  `required: true`) with no decision on your machine refuses the launch and
  names this command as the fix. A denied credential refuses distinctly.

Either way you choose to use the value Lens Sandbox detects on the host, store a
specific value at the boundary, or deny (requests carrying the placeholder then
fail at the boundary). A bound value is remembered for next time, and so is a
deny. Closing a card, by contrast, decides nothing at all: nothing is recorded,
the held request fails, and the next use asks again.

Denying is the one choice whose reach depends on which card you answered.
Denying the **proactive** card is a decision about the machine, and lands in
`~/.lns-credentials.json` alongside the values. Declining a **reactive**
first-use card is a decision about the workload in front of you, and is
remembered as a per-workload deny instead — see
[Workload grants](#workload-grants).

## Workload grants

Having a value bound on your machine is not the same as letting a *particular*
workload spend it. Binding is machine-wide; the decision to spend is per
workload. So a connector arms only where you have granted it — a copied
`lns-policy.yaml`, or a sandbox definition that declares a connector you happen
to have connected, still meets a first-use card rather than silently reaching
for the real secret.

A grant is scoped to the project, the workload, and the connector, and it
records the environment variable and injection domains the card disclosed to
you. If the connector is later redefined to use a different variable or
different domains, the grant no longer matches and you are asked again rather
than the new shape inheriting the old approval. That holds for a decline as
well as an approval — a redefined connector is a different question, so a
standing no does not carry over to it either.

Grants live in `~/.lns-workload-grants.json` — per-machine, alongside the
credential values and equally outside anything you commit. Declining a card is
remembered there too, as a standing no for that workload only; the same
connector is still offered to your other projects, and to other workloads in
this one.

Inspect and clear them with:

```
lns connector grants              # what this project granted or declined
lns connector grants --all        # every project on this machine
lns connector revoke <id>         # forget this project's grants for one connector
```

`lns connector disconnect <id>` forgets them too, as part of removing the
connector from the directory's policy. A revoke applies to the next run — a
sandbox already running keeps the arming it was granted at launch — but it does
win over a card that run is still holding: answering that card afterwards arms
the run in front of you and records nothing, so the forget stays forgotten and
the window tells you the decision was not remembered.
Connecting is not a way back: binding a value on the machine and granting a
workload are different decisions, so `lns connector connect <id>` leaves a
decline standing and tells you when this project holds one.

An [`oauth` connector](connectors.md)'s value decision is different in kind: rather
than a pasted secret it's obtained by an interactive **sign-in**
(`lns connector connect <id>`). A device-flow connector yields a self-renewing
**token set**, refreshed automatically and re-prompted when the grant can no longer be
refreshed; a pkce connector yields a **durable key** captured through your browser,
with no refresh or expiry. Either way it lives in the same per-machine file and is
never written to `lns-policy.yaml`.

When a workload asks for an `oauth` connector you are already signed in to, the
card offers both: grant the connection you have, or **reconnect** — sign in
again, which replaces it. Reconnecting is how you switch the account a workload
uses, or recover from a connection the service has revoked at its end.

## Injection kinds

A connector's injection `kind` decides how the real value reaches the destination:

| Kind                   | Effect on the request                                                 |
| ---------------------- | --------------------------------------------------------------------- |
| `bearer_header`        | `Authorization: Bearer <value>`                                       |
| `uri_placeholder`      | Value substituted into the request URI                                |
| `token_header`         | `Authorization: token <value>`                                        |
| `basic_x_access_token` | HTTP Basic auth as `x-access-token:<value>`                           |
| `api_key_header`       | A named header (the injection's `header:`, e.g. `x-api-key: <value>`) |

`lns connector add --inject` accepts all five non-AWS kinds. `api_key_header`
requires the header name as a third segment
(`--inject api_key_header:DOMAIN:HEADER`); the other four are headerless and use
the two-segment form (`--inject KIND:DOMAIN`). `awsSigv4` is unsupported as a
static placeholder — it carries real STS material.

## See also

- [Connectors](connectors.md) — declare, connect, and list the services whose
  credentials reach a workload.
- [Policy and approvals](policy.md) — value decisions follow the same
  allow / deny / ask model as network rules.
