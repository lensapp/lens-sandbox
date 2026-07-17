# Integrations

An **integration** teaches Lens Sandbox how to connect a workload to an external
service — bundling the service's credential injection *and* the network routes it
needs into one named, reusable unit. Connecting GitLab to a project, for example,
both allows `gitlab.com` and arranges for a `GITLAB_TOKEN` placeholder to be swapped
for your real token at the boundary (see [Credentials](credentials.md) for how
placeholders work).

## The catalog

The set of integrations Lens Sandbox knows about is a **catalog** with two layers:

- **Bundled** — ships inside `lns` and grows with each release, so common services
  work without any setup on your part.
- **User** — your own additions in `~/.lns-integrations.yaml`
  (override the path with `LNS_INTEGRATIONS_PATH`).

The effective catalog is the union of the two; a user entry can't shadow a bundled
id. List everything Lens Sandbox can connect:

```bash
lns integration list
```

### Declaring your own

For an internal service, declare an integration in your user catalog:

```bash
lns integration add acme \
  --env-var ACME_API_TOKEN \
  --inject bearer_header:api.acme.internal \
  --route api.acme.internal
```

- `id` — the integration id; must not collide with a bundled or existing user id.
- `--env-var` — the environment variable the placeholder is seeded into.
- `--inject KIND:DOMAIN` — how and where the real value is injected (repeatable); see
  the [injection kinds](credentials.md#injection-kinds).
- `--route HOST` — a host pattern the integration needs reachable (repeatable).
- `--placeholder` — a specific placeholder; auto-generated (self-identifying) when
  omitted.

Remove a user integration (bundled ones can't be removed):

```bash
lns integration remove acme
```

## Reaching a workload

An integration reaches a project's workloads in any of three ways:

- **Declared in the sandbox definition.** List its id under `spec.integrations`
  in [`./lns.yaml`](running-workloads.md#defining-a-sandbox). Declaring is
  disclosure and arming in one: anyone who runs the sandbox — from the local
  definition or a published copy — gets the integration armed at launch, with no
  per-directory setup. An id the machine's catalog doesn't know refuses the
  launch and points at `lns integration add`.
- **Required as a credential slot.** A definition's `spec.credentials` entry
  names an integration, the env var it is injected as (remapping the catalog
  default), and whether the workload requires it. A slot arms like a declared
  integration under the slot's env name. A **required** slot with no value
  bound on the machine refuses the launch before any microVM boots — naming
  the credential, its injection target, and the `lns integration connect` fix —
  and a credential you've denied refuses distinctly. An optional slot runs
  reactively. A required `oauth`-kind slot blocks on the sign-in instead.
- **Connected to the directory.** `lns integration connect` binds the
  integration's per-machine [value decision](credentials.md#value-decisions) —
  the approval-window card for a credential integration, the sign-in for an
  `oauth` one — and records the id in that directory's
  [`lns-policy.yaml`](policy.md), which is also how a directory with no
  definition arms an integration:

```bash
lns integration connect gitlab
lns integration disconnect gitlab
```

The policy stores the integration by id under `integrations:`, so the definition
resolves from the catalog at run time and the shareable policy stays small:

```yaml
network:
  allowedRoutes: []
  defaultVerdict: ask
integrations:
  - gitlab
```

The two sources union at launch. However an integration is armed — declared or
connected — its declared routes are allowed and its placeholder is seeded. The
first request carrying that placeholder follows the ordinary credential
[value decision](credentials.md#value-decisions) — it pauses for approval if you
haven't bound a value yet, where you choose to use the host value, store one, or
deny. A declared `oauth` integration with no sign-in on this machine blocks the
launch until the sign-in completes, so the workload never starts half-armed. A
new integration reaches a workload only at launch, so relaunch a running sandbox
to pick it up.

## The catalog file

The bundled and user catalogs share one schema, so an entry is portable between them:

```yaml
integrations:
  - id: gitlab
    authKind: credential
    routes:
      - match: gitlab.com
    credential:
      envVar: GITLAB_TOKEN
      placeholder: glpat-LNSPLACEHOLDER0000000000000000
      injections:
        - kind: api_key_header
          domain: gitlab.com
          header: PRIVATE-TOKEN
        - kind: bearer_header
          domain: gitlab.com
```

An entry may list several injections so one credential reaches a service however its
clients send it — here both the `PRIVATE-TOKEN` header `glab` uses and the
`Authorization: Bearer` form OAuth-style clients send.

A route may carry the same detail a [policy rule](policy.md#rules) can — a `scheme`
and HTTP method/path `rules` for least-privilege access — beyond the bare `match`.

`authKind` is `credential` or `oauth`. An `oauth` integration authenticates by an
interactive **sign-in** the background service drives for you — `lns integration
connect <id>` walks you through it and records the integration only once it completes,
and the obtained credential is injected at the boundary like any other, stored per
machine and never in `lns-policy.yaml`. An `oauth` entry carries an `oauth:` block (in
place of `credential:`) whose `flow` selects one of two shapes:

- **`flow: device`** (RFC 8628, the default) — `connect` prints a verification URL and
  a code, and you authorize in a browser. The token is short-lived, refreshed
  automatically, and a grant that can no longer be refreshed re-prompts the sign-in on
  next use. The bundled `github` integration signs in this way; its block carries a
  client id, scopes, and the device-authorization and token endpoints.
- **`flow: pkce`** (OAuth 2.0 authorization code + PKCE) — `connect` opens your browser
  to the provider's authorization page, and after you approve, the service captures the
  returned key over a one-time loopback callback. The result is a **durable** key with
  no refresh or expiry, so it stays armed across runs until the provider revokes it. The
  bundled `openrouter` integration signs in this way; its block carries an authorization
  endpoint and a token endpoint (and no client id).

## See also

- [Credentials](credentials.md) — how placeholders keep real secrets out of the
  workload, and the per-machine value decisions integrations reuse.
- [Policy and approvals](policy.md) — the `lns-policy.yaml` file that records which
  integrations a project has connected.
