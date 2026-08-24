# Connectors

A **connector** teaches Lens Sandbox how to connect a workload to an external
service — bundling the service's credential injection *and* the network routes it
needs into one named, reusable unit. Connecting GitLab to a project, for example,
both allows `gitlab.com` and arranges for a `GITLAB_TOKEN` placeholder to be swapped
for your real token at the boundary (see [Credentials](credentials.md) for how
placeholders work).

## The catalog

The set of connectors Lens Sandbox knows about is a **catalog** with two layers:

- **Bundled** — ships inside `lns` and grows with each release, so common services
  work without any setup on your part.
- **User** — your own additions in `~/.lns/connectors.yaml`.

The effective catalog is the union of the two; a user entry can't shadow a bundled
id. List everything Lens Sandbox can connect, how each signs in, and which ones
this project uses:

```bash
lns connector list
```

```text
CONNECTOR    SOURCE   SIGN-IN     CONNECTED
gitlab       bundled  credential  yes
github       bundled  oauth       no
acme         user     credential  no
```

Declaring a connector grants nothing: `CONNECTED` is the column that says whether
this project uses it.

### Declaring your own

For an internal service, declare a connector in your user catalog:

```bash
lns connector add acme \
  --env-var ACME_API_TOKEN \
  --inject bearer_header:api.acme.internal \
  --route api.acme.internal
```

One connector per domain: an `--inject` naming a destination another connector
already claims is refused, and the error names the one that holds it. Two
connectors claiming the same host would make the value a request carries
ambiguous.

- `id` — the connector id; must not collide with a bundled or existing user id.
- `--env-var` — the environment variable the placeholder is seeded into.
- `--inject KIND:DOMAIN` — how and where the real value is injected (repeatable); see
  the [injection kinds](credentials.md#injection-kinds).
- `--route HOST` — a host pattern the connector needs reachable (repeatable).
- `--placeholder` — a specific placeholder; auto-generated (self-identifying) when
  omitted.

Remove a user connector (bundled ones can't be removed):

```bash
lns connector remove acme
```

## Reaching a workload

A connector reaches a project's workloads in any of three ways:

- **Declared in the sandbox definition.** List its id under `spec.connectors`
  in [`./lns.yaml`](running-workloads.md#defining-a-sandbox). Declaring seeds
  the connector's placeholder env var — so a workload that checks for the
  variable starts up and attempts its first request — but never arms it: no
  route is opened and no bound value is injected on its behalf, even for a
  credential already bound on this machine. The workload is offered a live
  connect the first time it reaches the connector's domain; accepting it arms
  the connector and records the connection for this project, per machine. This
  is what keeps an untrusted published
  sandbox from spending a bound credential or opening a route behind your back.
  An id the machine's catalog doesn't know refuses the launch and points at
  `lns connector add`.
- **Asked for by a declared credential.** A definition's `spec.credentials`
  entry states the whole injection contract itself — the variable the workload
  reads, the placeholder it holds, and the domains the real value may travel to
  — and names no connector. A connector whose own claim covers one of those
  domains supplies the value: it is injected under the *declared* variable, with the
  *declared* placeholder, once this workload has been
  [granted](credentials.md#workload-grants) it; until then it is offered on
  first use like any other. With no connector claiming the domain the
  declaration still works — the workload holds the placeholder and the first
  request asks you for a value. A connector supplies one declaration, since it
  holds one value: a second entry naming the same domain asks you for its own.
  When the supplying connector is `oauth`-kind and holds no sign-in, the launch
  blocks on that sign-in before any microVM boots, and completing it is itself
  the
  [grant](credentials.md#workload-grants) — the workload starts armed rather
  than asking you twice.
- **Connected to the directory.** `lns connector connect` binds the
  connector's per-machine [value decision](credentials.md#value-decisions) —
  the approval-window card for a credential connector, the sign-in for an
  `oauth` one — and records the connection for that project, which is also how a
  directory with no definition connects one:

```bash
lns connector connect gitlab
lns connector disconnect gitlab
lns connector connect gitlab --project ../other-project
```

`--project <PATH>` acts on another project directory instead of the one you are
in; `connect`, `disconnect`, `grants` and `revoke` all take it.

The connection is recorded by id for the project, per machine, so the
definition resolves from the catalog at run time and nothing about which
connectors you use travels with a file you commit.

Only a connector you have **connected** to this directory opens its declared
routes and seeds its placeholder at launch, and the first request carrying that
placeholder follows the ordinary credential
[value decision](credentials.md#value-decisions) — where you choose to use the
host value, store one, or deny. Connecting is a property of the directory, not
of any one workload, so the value itself arms only where this workload holds a
[grant](credentials.md#workload-grants); a clone of the project asks again
rather than inheriting your approval. A **declared** id from a sandbox definition
seeds its placeholder but never arms on its own; it is offered reactively on
first use, so an untrusted published sandbox can't open a route or spend a
bound credential without your say-so. A connector supplying a declared
credential is the one case that can hold the launch: when it signs in with
`oauth` and this machine holds no sign-in, the run blocks on that sign-in
rather than starting half-provisioned. A new
connector reaches a workload only at launch, so relaunch a running sandbox to
pick it up.

## The catalog file

The bundled and user catalogs share one schema, so an entry is portable between them:

```yaml
connectors:
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

`authKind` is `credential` or `oauth`. An `oauth` connector authenticates by an
interactive **sign-in** the background service drives for you — `lns connector
connect <id>` walks you through it and records the connector only once it completes,
and the obtained credential is injected at the boundary like any other, stored per
machine and never in a file a project commits. An `oauth` entry carries an `oauth:` block (in
place of `credential:`) whose `flow` selects one of two shapes:

- **`flow: device`** (RFC 8628, the default) — `connect` prints a verification URL and
  a code, and you authorize in a browser. The token is short-lived, refreshed
  automatically, and a grant that can no longer be refreshed re-prompts the sign-in on
  next use. The bundled `github` connector signs in this way; its block carries a
  client id, scopes, and the device-authorization and token endpoints.
- **`flow: pkce`** (OAuth 2.0 authorization code + PKCE) — `connect` opens your browser
  to the provider's authorization page, and after you approve, the service captures the
  returned key over a one-time loopback callback. The result is a **durable** key with
  no refresh or expiry, so it stays armed across runs until the provider revokes it. The
  bundled `openrouter` connector signs in this way; its block carries an authorization
  endpoint and a token endpoint (and no client id).

## See also

- [Credentials](credentials.md) — how placeholders keep real secrets out of the
  workload, and the per-machine value decisions connectors reuse.
- [Policy and approvals](policy.md) — the `lns-local-mixin.yaml` file that records the
  destinations a project decided.
