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
  egress:
    http: []
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

`defaultVerdict` decides what happens to a request that no rule in `egress.http`
matches:

- `ask` (the default) — pause and prompt you.
- `allow` — let unmatched requests through.
- `deny` — block unmatched requests.

`ask` is the recommended starting point: you run into the policy as the workload
does real work, and your decisions accumulate into a least-privilege rule set.

### Rules

Each entry in `egress.http` is a rule:

```yaml
network:
  defaultVerdict: ask
  egress:
    http:
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
| `binaries`    | Optional list of guest binaries the rule is scoped to (see below).      |
| `rules`       | Optional `method` / `path` list; with it the rule permits only the requests it names and denies the rest. |

A `match` pattern can be:

- an exact host — `api.github.com`
- a wildcard — `*.github.com`
- a CIDR block — `10.0.0.0/8`
- a host with a port — `registry.internal:5000`

`egress.http` used to be a top-level `allowedRoutes:` list. That key is gone: a
policy file still naming it is refused with an error rather than loaded as a
policy with no rules. Move the list under `egress: http:`.

### Scoping a rule to specific binaries

`binaries` narrows a rule to the processes allowed to use it. Here only
`/usr/bin/git` reaches `git.example.test`; every other process in the sandbox is
denied that host:

```yaml
network:
  defaultVerdict: ask
  egress:
    http:
      - match: git.example.test
        verdict: allow
        binaries:
          - /usr/bin/git
```

This is the one policy feature that denies without asking, so read the rest of this
section before using it.

- **The filter fails closed.** A caller that is not on the list does not fall
  through to `defaultVerdict` — it is denied, and never prompted. Adding an
  unrestricted `allow` or `ask` for the same destination later does *not* re-open it
  for the excluded callers: the guest skips such a rule rather than let it undo the
  scoping. What it does not skip is another *scoped* rule, so that is how you grant a
  second binary — list them together in one rule, or add another scoped rule.
- **Only meaningful on `allow`.** On a `deny` rule the listed binaries are blocked by
  verdict and every other caller fails closed, so the only thing the scoping buys is
  that a later rule scoped to one of those others can still let it through. That is
  too fine a distinction to be worth the confusion, so `lns policy deny` has no
  `--binary` flag; a hand-written scoped deny is honoured, but read it as a plain
  deny with one narrow exception rather than as a per-binary block.
- **Absolute, canonical paths only.** The path is compared against the kernel's view
  of the running process (`/proc/<pid>/exe`), so name the real binary, not a symlink
  or a `PATH` shim. Paths that could never equal such a target are rejected when the
  policy loads: a relative path, one climbing through `..`, one naming no binary, and
  an empty `binaries: []` list — the last would match no caller and deny the
  destination outright.
- **Ancestors count, up to eight deep.** A caller matches if its own executable path
  or one of its first eight parents' is on the list, so a process that shells out
  still matches a rule naming the binary that launched it. The walk stops at the
  guest's init, at that eighth parent, or at the first parent whose executable it
  cannot read — a caller further down a wrapper chain than that is denied like any
  other unlisted one, without being asked.
- **It gates DNS too**, not only the connection itself.
- **Order matters.** Rules are read top to bottom and the first match wins, so a
  scoped rule placed after an unrestricted rule for the same destination never fires.
  `lns policy allow` handles this for you — it puts a new rule ahead of any existing
  rule that would pre-empt it and prints where it landed — but when you hand-edit the
  file, mind the order yourself.

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

### Scope an allow rule to a binary

`--binary` takes one absolute path and repeats:

```bash
lns policy allow git.example.test --binary /usr/bin/git
lns policy allow api.example.test --binary /usr/bin/curl --binary /usr/bin/wget
```

`lns policy deny` does not take `--binary` — see
[Scoping a rule to specific binaries](#scoping-a-rule-to-specific-binaries) for why.

Because the guest stops at the first matching rule, a narrowing rule only has an
effect ahead of the broader rule it narrows. `lns policy allow` places it there and
says so:

```
Added allow rule for "api.example.test" to lns-policy.yaml
Placed it before the existing rule for "*.example.test", which covers the same
destination and would otherwise pre-empt it. Every other caller is now denied
"api.example.test" without being asked, and that rule no longer serves them.
```

That second sentence is the fail-closed filter arriving: the rule the scoped one now
sits in front of is still in the file, but it no longer admits anyone for that
destination. A scoped rule for a destination no other rule covers has nothing to sit
in front of, so it is appended — and still reports what it now denies:

```
Added allow rule for "git.example.test" to lns-policy.yaml
Every other caller is now denied "git.example.test" without being asked.
```

A rule placed in front of another inherits its TLS termination, and says so. Sitting
ahead of a rule the sandbox intercepts is a narrowing of *who* may reach the
destination, not a request to stop intercepting it.

What it will not do is widen egress to get a rule to fire. Three cases are refused
outright, with nothing written:

- An `allow` for a destination an earlier `deny` already blocks. Ahead of that deny
  it would open every destination the deny covers, so narrowing the deny or
  reordering the file is left to you.
- An unrestricted `allow` for a destination an earlier binary-scoped rule already
  claimed. Ahead of that rule it would open the destination to every caller in the
  sandbox — the opposite of what the scoped rule says — so the error names the scoped
  rule and, if you did mean to open it up, tells you to drop that rule first:

  ```
  error: the rule for "git.example.test" is scoped to /usr/bin/git, and placing
  this allow rule in front of it would open the destination to every caller in the
  sandbox — drop the scoped rule with `lns policy remove git.example.test` first if
  that is what you mean
  ```

- An `allow` for a destination an earlier rule permits only *some requests* to (a
  rule carrying a `rules:` list). Ahead of that rule it would hand its callers every
  method and path the restriction was written to exclude.

A `deny` behind a deny is not an error: the destination is already blocked, so the
command says so and changes nothing. Adding a rule the file already holds is the
same — it reports and leaves the file alone. What counts as already held is the rule
the gate would actually reach, not any copy sitting somewhere in the file: a copy
stranded behind a rule that pre-empts it — by verdict, by binary scope, or by a
request filter — is not in force, so the command treats it as the placement or refusal
it is rather than reporting a grant the sandbox does not honour. Where the stranded
copy is the one that gets moved into force, it keeps the note it was carrying. The one
exception is `--description`: the note is not part of the grant, so passing a new one
for a rule the file already holds edits that rule in place instead of adding a second
copy of it.

One placement the CLI cannot make for you: a rule whose destination is an address
range or IP literal is compared numerically, but whether a *hostname* rule resolves
into a range is DNS's answer, not the file's. A scoped rule for a host behind a
broad CIDR allow is left where you put it, so order those by hand.

### List rules

```bash
lns policy list
```

The `BINARIES` column shows what each rule is scoped to, and `--format json` reports
the same under `binaries` (`null` for a rule open to every caller).

### Remove a rule

Remove every rule matching a destination pattern:

```bash
lns policy remove api.github.com
```

Removal goes by pattern alone, so it deletes *every* rule for that destination —
binary-scoped ones included. The command reports how many rules went; run
`lns policy list` first to see which ones they will be.

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
