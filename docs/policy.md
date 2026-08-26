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

One directory, one policy file — it sits next to the project it governs, and
nothing names it. A definition you run from another directory is governed by
that directory's file, not by the one where you typed the command.

### What the file decides, and what it doesn't

The file holds the decisions you have made. A destination no rule decides is one
you are asked about — that is not a setting, it is what happens when nothing
answers. So a fresh file decides nothing and asks about everything, and your
decisions accumulate into a least-privilege rule set as the workload does real
work.

To close a directory instead — block what you have not listed, without being
prompted — end `egress.http` with a catch-all deny:

```yaml
spec:
  egress:
    http:
      - match: "*"
        verdict: deny
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
  too fine a distinction to rely on; a scoped deny is honoured, but read it as a
  plain deny with one narrow exception rather than as a per-binary block.
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
  Put the specific rule first.

## Editing the file

No command edits the rules. A decision is recorded one of two ways: you answer the
card a run raises, or you open `lns-local-mixin.yaml` and write the rule yourself.
Both write the same document, and because it is a `kind: mixin` document,
`lns artifact validate -f lns-local-mixin.yaml` checks what you wrote and
`lns artifact inspect -f lns-local-mixin.yaml` renders it.

### Add an allow or deny rule

Append an entry to `egress.http`. The gate stops at the first match, so put a
specific rule ahead of the broad one it narrows:

```yaml
spec:
  egress:
    http:
      - match: api.github.com
        verdict: allow
        description: GitHub REST API
      - match: "*.npmjs.org"
        verdict: allow
      - match: metrics.vendor.example
        verdict: deny
```

### Scope an allow rule to a binary

`binaries` takes absolute paths, and the rule has to sit ahead of any broader rule
covering the same destination — behind one it never fires:

```yaml
spec:
  egress:
    http:
      - match: api.example.test
        verdict: allow
        binaries:
          - /usr/bin/curl
          - /usr/bin/wget
      - match: "*.example.test"
        verdict: allow
```

A scoped rule in front of a broader one denies every other caller that destination
without asking, and the broader rule no longer serves them. That is the fail-closed
filter arriving, and it is the point of the scoping — see
[Scoping a rule to specific binaries](#scoping-a-rule-to-specific-binaries).

Two orderings are worth avoiding, because the rule you write then does nothing:

- An `allow` behind a `deny` that already covers the destination. Narrow the deny
  rather than add an allow after it.
- An unrestricted `allow` behind a binary-scoped rule for the same destination. The
  guest skips such a rule rather than let it undo the scoping, so drop the scoped
  rule if you did mean to open the destination to every caller.

One ordering the file cannot settle for you: a rule whose destination is an address
range or IP literal is compared numerically, but whether a *hostname* resolves into
that range is DNS's answer. Order a scoped host rule against a broad CIDR allow by
hand.

### Add a raw TCP rule

`egress.tcp` entries take the same fields, and each must name a port:

```yaml
spec:
  egress:
    tcp:
      - match: db.internal:5432
        verdict: allow
        description: project database
        binaries: ["/usr/bin/psql"]
      - match: 10.0.0.0/8:5432
        verdict: deny
```

You only need these to decide a raw destination up front; one you have not decided
raises a card on first use. On a raw destination the caller scope is the whole of
the control — nothing between the workload and the destination can read the traffic.

Because `tcp` is the pre-filter, a raw rule also takes its destination over from the
`egress.http` rules naming that host on that port: those rules stop applying to it,
a `deny` among them included. Check what you are lifting before you add one.

### Read the rules

The summary a run prints before booting lists every rule in force and names the file
each came from, so `lns run` is where you read the merged result — including the
rules a mixin or the sandbox document contributed, which this file does not hold.

### Closing a directory

To block everything you have not listed, and stop being asked, end `egress.http`
with a catch-all deny:

```yaml
spec:
  egress:
    http:
      - match: api.github.com
        verdict: allow
      - match: "*"
        verdict: deny
```

The gate stops at the first matching rule, so the rules above it still decide the
destinations they name — the catch-all only answers for whatever is left.

Closing `egress.http` closes raw traffic too, so `egress.tcp` needs no counterpart:
a raw destination no `tcp` rule names falls through to the catch-all, and a
connection Lens Sandbox cannot read is refused rather than raising a card.

Two things worth knowing:

- **A near-catch-all is not a catch-all.** `deny 0.0.0.0/0` or `deny '*.com'` are
  ordinary rules covering a lot; only an unscoped `match: "*"` closes a directory.
  A `"*"` deny narrowed by `binaries` or a request filter decides only what it
  names, so it does not close anything either.
- **A closed directory raises no approval cards**, so editing the file is how you
  widen it. That is deliberate: a card you never see cannot be a decision.

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

A [`pre-start` script](running-workloads.md#pre-start-scripts--preparing-the-guest)
raises the same card, before the workload has started. A script is gated exactly
as the workload is — same rules, same proxy, same decisions file — so a document
that installs from a package mirror ships that mirror's `egress` alongside the
script, and a run whose first card names `deb.debian.org` is a script asking, not
the workload.

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

Because policy is a plain file, it travels. Commit `lns-local-mixin.yaml` to the
repo so everyone running the project shares the same rules, or hand it to a
teammate to drop beside their own document. A run loads the file at startup, so
shared approvals are already in place — no one has to re-approve them.

## See also

- [Running workloads](running-workloads.md) — the run summary and what it lists.
- [CLI reference](cli-reference.md) — every command and flag as it ships today.
