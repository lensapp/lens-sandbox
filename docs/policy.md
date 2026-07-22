# Policy and approvals

A policy decides what a workload may reach over the network. Lens Sandbox checks
every outbound connection the workload opens against the policy and does one of
three things: **allow** it, **deny** it at the boundary, or — when no rule matches
— **ask** you.

## The policy file

Policy lives in a human-readable YAML file. By default that's `lns-policy.yaml` in
the directory you run from; the first `lns run` in a directory without one creates
it:

```yaml
network:
  allowedRoutes: []
  defaultVerdict: ask
```

`lns run` writes this starter file for you, so you only deal with it by hand if you
choose to edit the file directly.

One directory, one policy file — it sits next to the project it governs. Point a
run at a different file with `--policy <path>`:

```bash
lns run --policy ~/team/shared-policy.yaml ghcr.io/acme/agent
```

### Default verdict

`defaultVerdict` decides what happens to a request that no rule in `allowedRoutes`
matches:

- `ask` (the default) — pause and prompt you.
- `allow` — let unmatched requests through.
- `deny` — block unmatched requests.

`ask` is the recommended starting point: you run into the policy as the workload
does real work, and your decisions accumulate into a least-privilege rule set.

### Rules

Each entry in `allowedRoutes` is a rule:

```yaml
network:
  defaultVerdict: ask
  allowedRoutes:
    - match: api.github.com
      verdict: allow
      description: GitHub REST API
    - match: "*.telemetry.example"
      verdict: deny
    - match: 10.0.0.0/8
      verdict: allow
```

| Field         | Meaning                                                                 |
| ------------- | ----------------------------------------------------------------------- |
| `match`       | Destination pattern (see below).                                        |
| `verdict`     | `allow`, `deny`, or `ask`.                                              |
| `description` | Optional human-readable note kept alongside the rule.                   |

A `match` pattern can be:

- an exact host — `api.github.com`
- a wildcard — `*.github.com`
- a CIDR block — `10.0.0.0/8`
- a host with a port — `registry.internal:5000`

## Editing rules from the CLI

You can hand-edit the YAML, but `lns policy` edits it for you and keeps it
well-formed. All subcommands default to `lns-policy.yaml` in the current directory;
pass `--policy <path>` to target another file.

### Add an allow or deny rule

```bash
lns policy allow api.github.com --description "GitHub REST API"
lns policy allow "*.npmjs.org"
lns policy deny metrics.vendor.example
```

### List rules

```bash
lns policy list
```

### Remove a rule

Remove the rule matching a destination pattern:

```bash
lns policy remove api.github.com
```

## The approval flow

When the workload makes a request that hits a `defaultVerdict: ask` (or a rule
whose verdict is `ask`), the request pauses and the background service shows an
approval window with the host and the action — for example
`CONNECT api.linear.app:443`. Your options:

- **Allow once** / **Deny once** — apply to this request only; nothing is written
  to the policy file.
- **Allow always** / **Deny always** — apply now *and* write a matching rule to
  the policy file, so the same question isn't asked again.

A denied request fails at the boundary the way a genuine network error would (a
refused connection, a failed DNS lookup, an HTTP error). The workload never gets a
silent fallback or a fake success.

If no one responds, the request times out and is treated as a denial.

## Sharing policy

Because policy is a plain file, it travels. Commit `lns-policy.yaml` to the repo so
everyone running the project shares the same rules, hand it to a teammate, or keep
a curated file somewhere central and point runs at it with `--policy`. A run loads
the file at startup, so shared approvals are already in place — no one has to
re-approve them.

## See also

- [Credentials](credentials.md) — credential decisions live alongside network
  policy and follow the same allow / deny / ask model.
- [Connectors](connectors.md) — connecting a connector records it under
  `connectors:` and allows the routes it declares.
- [Running workloads](running-workloads.md) — `--policy` and the run summary.
- [CLI reference](cli-reference.md) — the full `lns policy` flag list.
