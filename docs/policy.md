# Policy and approvals

A policy decides what a workload may reach over the network. Lens Sandbox checks
every outbound connection the workload opens against the policy and does one of
three things: **allow** it, **deny** it at the boundary, or — when no rule matches
— **ask** you.

## The policy file

Policy lives in a human-readable YAML file. By default that's
`lns-local-mixin.yaml` beside the definition being run — the directory you run
from, unless you name a definition in another one — and the first `lns run` in a
directory without one creates it:

```yaml
apiVersion: lns.run/v1
kind: mixin
name: lns-local-mixin
spec:
  egress:
    http: []
    tcp: []
```

`lns run` writes this starter file for you, so you only deal with it by hand if you
choose to edit the file directly.

The file is a **mixin** — the same document format a
[sandbox definition](running-workloads.md#defining-a-sandbox) layers on. Every
run in the directory resolves it without being named, after every other source,
so nothing you pulled overrules what you decided. Because it is a source like any
other, the summary a run prints before booting lists its rules and names the file
they came from — so a rule of yours that overrules one the sandbox shipped is
visible while you can still stop the run.

The rules the run enforces are folded from this file as it stands, not from a copy
taken at launch: a rule an approval appends mid-run applies at once, and one you
delete mid-run stops applying.

Being a mixin also means the file is not limited to `egress`. Any block a mixin
can declare, this one can declare too — a tool, a mount, another mixin to layer
on — and it reaches every run in this directory:

```yaml
apiVersion: lns.run/v1
kind: mixin
name: lns-local-mixin
spec:
  tools:
    - ripgrep@14
  egress:
    http: []
    tcp: []
```

One directory, one policy file — it sits next to the project it governs. Point a
run at a different file with `--policy <path>`:

```bash
lns run --policy ~/team/shared-policy.yaml ghcr.io/acme/agent
```

### What the file decides, and what it doesn't

The file holds the decisions you have made. A destination no rule decides is one
you are asked about — that is not a setting, it is what happens when nothing
answers. So a fresh file decides nothing and asks about everything, and your
decisions accumulate into a least-privilege rule set as the workload does real
work.

To close a directory instead — block what you have not listed, without being
prompted — end `egress.http` with a catch-all deny:

```bash
lns policy deny '*'
```

That rule governs raw destinations too, so `egress.tcp` needs no counterpart. See
[Closing a directory](#closing-a-directory).

### Rules

Each entry in `egress.http` is a rule:

```yaml
spec:
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
| `verdict`     | `allow` or `deny`. There is no `ask`: a destination no rule decides is asked about already. |
| `description` | Optional human-readable note kept alongside the rule.                   |
| `binaries`    | Optional list of guest binaries the rule is scoped to (see below).      |
| `rules`       | Optional `method` / `path` list; with it the rule permits only the requests it names and denies the rest. |

A `match` pattern can be:

- an exact host — `api.github.com`
- a wildcard — `*.github.com`
- a CIDR block — `10.0.0.0/8`
- a host with a port — `registry.internal:5000`

### Raw TCP destinations

Not everything a workload connects to speaks HTTP. A Postgres client, a Redis
client, an SSH session — Lens Sandbox cannot read those, so it cannot apply
`egress.http` rules to them. `egress.tcp` is where you name the ones you want to
allow anyway:

```yaml
spec:
  egress:
    http:
      - match: api.github.com
        verdict: allow
    tcp:
      - match: db.internal:5432
        verdict: allow
        description: project database
```

A connection matching a `tcp` rule is **spliced through untouched**: no TLS
interception, no HTTP rules, no credential injection, and no per-request record
in the audit chain — only the connection itself. Everything else about the rule
works the way an `egress.http` rule does, first match wins.

Three things are worth knowing:

- **`tcp` is a pre-filter, not a second policy.** It is consulted before
  `egress.http`. A destination no `tcp` rule names is untouched by the table and
  goes on to be matched, inspected and gated exactly as before. An empty `tcp`
  list changes nothing.
- **Every `tcp` rule must name a port.** `db.internal` is rejected;
  `db.internal:5432` is a rule. Because the traffic is passed through unread,
  "any port on this host" is not a grant Lens Sandbox will write. IPv6 uses
  bracket notation: `[2001:db8::1]:5432`, `[2001:db8::/32]:5432`.
- **You do not have to declare one to be asked.** A connection Lens Sandbox cannot
  read — Postgres, Redis, SSH — cannot be matched against `egress.http` rules at
  all, so rather than dropping it silently, it raises an approval card naming the
  address and port. Answering "always allow" writes the `egress.tcp` rule for you.
  A closed directory (a catch-all deny, see [Closing a directory](#closing-a-directory))
  refuses those connections instead of asking.

An optional `binaries:` list scopes a rule to specific callers, matched against
the guest's `/proc/<pid>/exe`, so the paths must be absolute:

```yaml
    tcp:
      - match: db.internal:5432
        verdict: allow
        binaries: ["/usr/bin/psql"]
```

A `tcp:` block makes the file unreadable by an lns older than the release that
introduced it. It fails loudly rather than silently dropping the rule, which for
a rule that grants uninspected traffic is the right way round.

### Scoping a rule to specific binaries

`binaries` narrows a rule to the processes allowed to use it. Here only
`/usr/bin/git` reaches `git.example.test`; every other process in the sandbox is
denied that host:

```yaml
spec:
  egress:
    http:
      - match: git.example.test
        verdict: allow
        binaries:
          - /usr/bin/git
```

This is the one policy feature that denies without asking, so read the rest of this
section before using it.

- **The filter fails closed.** A caller that is not on the list is not asked about
  — it is denied outright. Adding an unrestricted `allow` for the same destination
  later does *not* re-open it
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
well-formed. All subcommands default to `lns-local-mixin.yaml` in the current directory;
pass `--policy <path>` to target another file, including the one governing a
definition you run from elsewhere.

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
Added allow rule for "api.example.test" to lns-local-mixin.yaml
Placed it before the existing rule for "*.example.test", which covers the same
destination and would otherwise pre-empt it. Every other caller is now denied
"api.example.test" without being asked, and that rule no longer serves them.
```

That second sentence is the fail-closed filter arriving: the rule the scoped one now
sits in front of is still in the file, but it no longer admits anyone for that
destination. A scoped rule for a destination no other rule covers has nothing to sit
in front of, so it is appended — and still reports what it now denies:

```
Added allow rule for "git.example.test" to lns-local-mixin.yaml
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

### Add a raw TCP rule

The `-tcp` verbs write to `egress.tcp` instead, and require a port:

```bash
lns policy allow-tcp db.internal:5432 --description "project database"
lns policy deny-tcp  10.0.0.0/8:5432
```

You only need these to decide a raw destination up front; one you have not decided
raises a card on first use. `allow-tcp` takes `--binary` the way `allow` does, which on a raw
destination is the narrowest the grant gets — nothing between the workload and it can
read the traffic, so the one caller allowed to open it is the whole of the control:

```bash
lns policy allow-tcp db.internal:5432 --binary /usr/bin/psql
```

The raw table is first-match-wins, so these place a rule where it will actually
fire — in front of any rule already covering the same destination, saying so when
they do — and refuse a rule an existing raw deny would make dead.

Because `tcp` is the pre-filter, a raw rule also takes its destination over from the
`egress.http` rules naming that host on that port: those rules stop applying to it.
The `-tcp` verbs say which ones, and refuse outright when one of them is a `deny` —
lifting a block you wrote is a widening of your own policy, so narrow or remove that
deny first if that is what you mean.

### List rules

```bash
lns policy list
```

The `TABLE` column says which table each rule is in — `http` for inspected
routes, `tcp` for raw splices. The `BINARIES` column shows what each rule is scoped
to. `--format json` reports both, under `table` and `binaries` (`null` for a rule
open to every caller).

### Remove a rule

Remove every rule matching a destination pattern, from either table:

```bash
lns policy remove api.github.com
lns policy remove db.internal:5432
```

Removal goes by pattern alone, so it deletes *every* rule for that destination —
binary-scoped ones included. The command reports how many rules went; run
`lns policy list` first to see which ones they will be.

### Closing a directory

To block everything you have not listed, and stop being asked, end `egress.http`
with a catch-all deny:

```bash
lns policy deny '*'
```

The gate stops at the first matching rule, so the rules above it still decide the
destinations they name — the catch-all only answers for whatever is left. That is
why `lns policy allow <host>` still works afterwards: the allow goes in front of
the catch-all, and the command says so. A deny you aimed at a destination is
different, and an allow behind *that* is refused rather than reordered.

Closing `egress.http` closes raw traffic too, so `egress.tcp` needs no
counterpart: a raw destination no `tcp` rule names falls through to the catch-all,
and a connection Lens Sandbox cannot read is refused rather than raising a card.

Three things worth knowing:

- **A second catch-all replaces the first.** `lns policy allow '*'` reopens a closed
  directory by replacing the catch-all deny, and `lns policy deny '*'` closes an open
  one the same way. Two catch-alls cannot both be in force, so the file is never left
  carrying one the gate never reaches.
- **A near-catch-all is not a catch-all.** `deny 0.0.0.0/0` or `deny '*.com'` are
  ordinary rules covering a lot; only an unscoped `match: "*"` closes a directory.
  A `"*"` deny narrowed by `binaries` or a request filter decides only what it
  names, so it does not close anything either.
- **A closed directory raises no approval cards**, so the CLI is how you widen it.
  That is deliberate: a card you never see cannot be a decision.

## The approval flow

When the workload reaches a destination no rule decides, the request pauses and the
background service shows an approval window with the host and the action — for example
`CONNECT api.linear.app:443`. Your options:

- **Allow once** / **Deny once** — apply to this request only; nothing is written
  to the policy file.
- **Allow always** / **Deny always** — apply now *and* write a matching rule to
  the policy file, so the same question isn't asked again. The rule carries the
  note `approved during a run`, so a destination you answered for reads
  differently from one somebody wrote by hand.

**Always** writes a rule only where the guest would reach it. If some rule already
decides that destination — an earlier answer, or a rule you wrote — the gate stops
there, so a second one behind it would never fire. Nothing is written and the
approval window says the decision applied to that request alone. Same if the file
already holds the very rule the answer would write, stranded behind another rule the
gate reaches first: rather than reorder a file you wrote, the window tells you where
the rule is so you can move it ahead yourself.

A card for a connection Lens Sandbox cannot read is marked **RAW** and says
so in as many words: Lens Sandbox cannot inspect that traffic or inject
credentials into it. Allowing it always writes a port-scoped rule —
`db.internal:5432`, never `db.internal` — and names any `egress.http` rule that
stops applying to that traffic, since a raw splice is read by nothing.

A denied request fails at the boundary the way a genuine network error would (a
refused connection, a failed DNS lookup, an HTTP error). The workload never gets a
silent fallback or a fake success.

If no one responds, the request times out and is treated as a denial.

## Sharing policy

Because policy is a plain file, it travels. Commit `lns-local-mixin.yaml` to the repo so
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
