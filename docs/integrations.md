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
- `--inject KIND:DOMAIN` — how and where the real value is injected (repeatable); the
  same kinds as [custom credential providers](credentials.md#injection-kinds).
- `--route HOST` — a host pattern the integration needs reachable (repeatable).
- `--placeholder` — a specific placeholder; auto-generated (self-identifying) when
  omitted.

Remove a user integration (bundled ones can't be removed):

```bash
lns integration remove acme
```

## Connecting

An integration only affects a project once you **connect** it, which records it in
that directory's [`lns-policy.yaml`](policy.md):

```bash
lns connect gitlab
lns disconnect gitlab
```

The policy stores the integration by id under `integrations:`, so the definition
resolves from the catalog at run time and the shareable policy stays small:

```yaml
network:
  allowedRoutes: []
  defaultVerdict: ask
  defaultTransport: direct
integrations:
  - gitlab
```

When a connected integration's run starts, its declared routes are allowed and its
placeholder is seeded. The first request carrying that placeholder follows the
ordinary credential [value decision](credentials.md#value-decisions) — it pauses for
approval if you haven't bound a value yet, exactly like a built-in provider. Set one
ahead of time with `lns credential set <id>`. A new integration reaches a workload
only at launch, so relaunch a running sandbox to pick it up.

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
        - kind: bearer_header
          domain: gitlab.com
```

A route may carry the same detail a [policy rule](policy.md#rules) can — a `scheme`
and HTTP method/path `rules` for least-privilege access — beyond the bare `match`.

`authKind` is `credential` today. `oauth` (an interactive sign-in that obtains the
token for you, rather than you supplying it) is reserved for a future release;
connecting an `oauth` integration isn't supported yet.

## See also

- [Credentials](credentials.md) — how placeholders keep real secrets out of the
  workload, and the per-machine value decisions integrations reuse.
- [Policy and approvals](policy.md) — the `lns-policy.yaml` file that records which
  integrations a project has connected.
