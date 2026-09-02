# Sandbox definition specification — `lns.run/v1`

Normative reference for the `lns.run/v1` document format: the `lns.yaml` a
developer writes, the mixin that layers a capability onto a sandbox, and the
connector the user installs to reach a service.

> **Status: the target, not today.** This document is the agreed destination for
> the format. It describes that destination as settled — nothing below hedges
> about implementation state — and parts of it do not ship yet.
>
> For what `lns` accepts today, read the guides:
> **[Running workloads](running-workloads.md)** for authoring and publishing,
> and **[Policy and approvals](policy.md)** for the network rule grammar and a
> run's own decisions.
>
> The product is pre-1.0. Every gap between this document and the code closes as
> a breaking, unversioned change with no compatibility shim.

For the task-oriented authoring guide, see
[Running workloads](running-workloads.md#defining-a-sandbox). For the network
rule grammar, see [Policy and approvals](policy.md).

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, and **MAY** are
used as described in RFC 2119.

---

## 1. Overview

An **artifact** is one YAML document with an `apiVersion`, a `kind`, a `name`,
and a `spec`, published as one OCI artifact. Three artifact kinds exist:

| Kind | Purpose |
|---|---|
| **`sandbox`** | A complete sandbox: the base image, the egress it needs, the credentials it needs, and the files, tools, and mounts it brings. |
| **`mixin`** | A capability layered onto a sandbox: tools, filesets, egress, credentials. |
| **`connector`** | A mixin the user installs on their machine, offered when a run reaches a service it covers. It carries only what can be applied to a guest that is already running, and it applies only after the user grants it. |

[Chapter 3](#3-artifact-kinds) specifies each in full.

### 1.1 One distribution mechanism

Every artifact is published and consumed the same way: **one document, one OCI
artifact, addressed by reference and pinned by digest** ([§7](#7-distribution)).

The uniformity is what makes artifacts composable. A sandbox reference, a
connector reference, and a mixin reference are the same kind of thing to a
reader, to `lns pull`, and to an approval — each independently versioned, each
auditable at its digest, each fixable by publishing a new one.

`lns run` boots a sandbox. `lns push` publishes any artifact. `lns pull` and
`lns run <reference>` consume the published form. The same document is both the
authoring surface and the wire format.

### 1.2 Strict decoding

Decoding rejects unrecognized fields at every level of every document. Misspell a
field — `guestpath` for `guestPath`, or `egres` for `egress` — and the load fails
on that line instead of ignoring the key and running with a default. `lns sandbox
validate` runs the whole schema and cross-field check offline — no service, no
network.

One stated exception exists, and it is one map deep: the body of a connector
method's `auth` whose `kind` this reader does not know is not decoded, so the
document loads and the method is simply not offered
([§3.2.2](#322-methods)).

### 1.3 Disclosure before boot

A run does what its artifact says. `spec.egress` is **enforced as written** — an
entry that allows a destination allows it, and a mixin that narrows one narrows
it. The safeguard is not that a document is advisory; it is that the developer
sees the resolved document, in full, before anything boots
([§1.5](#15-one-disclosure)).

The same holds for `spec.ports`: a declared port is published, whether the
sandbox came from a registry or from the directory in front of you.

Three things stay outside a document's reach, because no declaration can supply
them:

- **A destination nothing decides.** An entry decides its match; a destination no
  entry covers is asked about at first use, so silence is never an allow.
- **A secret.** `spec.credentials` names the variable and the domains a value may
  travel to, and nothing more. The value is bound per machine.
- **A file on the machine that runs it.** A `filesets[].hostPath`
  ([§3.1.11](#3111-filesets)) names one, and a pulled document only names it —
  whether the file is read is decided per machine, on the same terms as a
  destination. A document in the developer's own directory is their own consent
  and is never asked about.

A `connector` is the one kind that grants nothing even after it arrives.
Installing it opens no destination, arms no injection, writes no file, and
supplies no real value — it makes the destinations it serves ask. It lends its
placeholder to a variable a sandbox itself declared and left to it
([§3.1.7](#317-credentials)), and sets nothing a sandbox did not ask for. What
grants is the user granting one of its methods to a run
([§3.2.4](#324-installing-connecting-and-applying)).

### 1.4 Credentials, and what a connector adds

A **credential** is the mechanism ([§4.1](#41-the-credential-definition)). It is the
single description the proxy works from: this placeholder, in this variable,
replaced by the real value on a request to this domain. Nothing else injects a
secret, so a credential is the truth about where a secret can go, and any kind
declares as many as it needs.

A credential is complete on its own. Given one, the run knows what to ask the
developer for — a token, pasted once — and knows exactly where the value may
travel afterwards.

A **connector** ([§3.2](#32-kind-connector)) is a mixin that carries one, plus
the egress that reaches the service and the files the service's client expects —
each stated inside a **method**, one of the alternative ways the connector can be
connected ([§3.2.2](#322-methods)). It is worth having for four reasons a bare
credential cannot cover:

| A connector adds | Why it matters |
|---|---|
| One or more credentials, published together | The env var, placeholder, and injection kinds for a service are stated once by whoever knows them, instead of by every author who talks to it. |
| The egress that service needs | Reaching the destination and authenticating to it are one decision, so connecting arrives with its own egress rather than a second round of approvals. |
| The files its client reads | A tool that looks for `~/.claude/.credentials.json` rather than an environment variable is served the same way, and by the same document. |
| The ways to authenticate | A service reached with a pasted token and the same service reached with a browser sign-in need different variables, different files, and sometimes a different transport. A connector states each as a **method**, and the user picks one. |

So a connector is a convenience and a security upgrade over pasting, never a
different mechanism. What supplies the value is still the user's decision on
their own machine.

**Two things separate a connector from a plain mixin, and neither is the
format.** A mixin is named by a document and resolved at startup. A connector is
named by nobody: the user installs it, a run **detects** that it is relevant, and
the user is **offered** it. Detection reads `serves` — the destinations the
connector is worth offering for — and, before boot, the `envVar` of each
credential its methods declare, which is how a run finds the connector a
declaration was left to ([§3.1.7](#317-credentials)). Neither reading grants
anything.

**The restriction is what makes it a mixin at all.** A connector is applied while
the guest is already running, so it may carry only blocks that a booted guest can
still be given — egress, credentials, filesets, and env — and no others
([§3.2.3](#323-what-a-method-may-carry)). What it carries merges by the ordinary
rules in [§3.3.2](#332-merge-rules), so a method granted at the gate and one
already granted at boot produce the same run.

**Nothing applies until the user grants a method.** Installing is not a merge: it
grants nothing, and makes the connector's destinations ask. The one thing it sets
is a marker in a variable a sandbox itself declared and left to it
([§3.1.7](#317-credentials)).
A **grant** applies one method to one run; a **connect** authenticates where a
method requires it, and the card does both at once
([§3.2.4](#324-installing-connecting-and-applying)).

### 1.5 One disclosure

A consumer approves one thing before an untrusted sandbox runs, and composition
MUST preserve that.

A mixin resolves into the sandbox at **startup**, so the document a consumer
pulls is not yet the sandbox that boots. What the consumer approves is therefore
the **resolved** sandbox: before boot, the run presents the merged result in full
— every rule, mount, file, tool, script, and credential, each attributed to the
mixin it came from. A declared credential left to a connector
([§3.1.7](#317-credentials)) names that connector too, marked as supplying no
value yet: the variable is set from a document the consumer did not approve, so
the approval must say whose marker it holds. Whatever the reference looked like, the approval is against a resolved
digest, so what boots is what was approved
([§3.3.1](#331-how-a-mixin-enters-a-run)).

Startup resolution is what makes a mixin worth having: a fix to a mixin reaches
every sandbox that references it on the next run, without republishing any of
them. See [§3.3](#33-kind-mixin).

---

## 2. Common top-level fields

Every artifact sets these, whatever its kind.

```yaml
apiVersion: lns.run/v1      # REQUIRED. Exactly this string.
kind: sandbox               # REQUIRED. sandbox | connector | mixin.
name: reviewer              # REQUIRED. ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$
spec: { … }                 # REQUIRED. Shape depends on kind.
```

| Field | Type | Rules |
|---|---|---|
| `apiVersion` | string | REQUIRED. MUST be `lns.run/v1`. It versions this grammar, so a reader knows how to parse the rest before it starts. |
| `kind` | string | REQUIRED. `sandbox`, `connector`, or `mixin`. |
| `name` | string | REQUIRED. A DNS label: lowercase alphanumeric and `-`, 1–63 characters, first and last character alphanumeric. For a `connector` this is the connector id. |
| `spec` | map | REQUIRED. Identity sits above it and content inside it, so a block added later can never collide with a key that names the document. |

**An artifact carries no version of its own.** The reference it was pushed to is
the release — `ghcr.io/acme/reviewer:1.4.0`, and the digest under it — so a
`version` field would be a second answer to the same question, free to disagree
with the tag it shipped under. Nothing in this document reads one: resolution,
approval, and merging all key on the digest. See [§7](#7-distribution).

The same name pattern applies to every identifier in every document: connector
ids, tool names, and mixin names.

**One API group.** Every artifact is `lns.run/v1`, and the three kinds are all of
them — so one reader, and one grammar, covers everything a machine pulls or a
developer writes.

[Chapter 3](#3-artifact-kinds) specifies the three kinds.
[Chapter 4](#4-shared-definitions) defines the two `spec` shapes they share.

---

## 3. Artifact kinds

An **artifact** is one published document of one kind. The three kinds are the
three things a user assembles a run from, and the term names what they have in
common: each is authored, published, versioned, and approved the same way
([§1.1](#11-one-distribution-mechanism)).

| Kind | Whose document it is | How it enters a run |
|---|---|---|
| [`sandbox`](#31-kind-sandbox) | The workload's author | `lns run <reference>`, or a local `./lns.yaml`. Exactly one. |
| [`mixin`](#33-kind-mixin) | Anyone | Referenced by a sandbox, merged at startup. |
| [`connector`](#32-kind-connector) | The user, on their machine | Installed once, then offered when a run reaches a service it covers. Merged when the user grants it. |

The middle column is the distinction to keep in view. A `sandbox` and a `mixin`
travel **with the workload** — an author writes them, and a consumer approves what
they say. A `connector` belongs to **the machine**: the user installs it, and no
sandbox can name one ([§1.4](#14-credentials-and-what-a-connector-adds)) — which
is why it is the one kind a run merges without any document having asked for it.
All three are artifacts because they are published and pinned alike, not because
the same person writes them.

### 3.1 `kind: sandbox`

A complete sandbox, with every field carrying a real value:

```yaml
apiVersion: lns.run/v1
kind: sandbox
name: reviewer
spec:
  image: ghcr.io/acme/reviewer@sha256:5f0c9a1d3b7e42a86c15d0f9b3e7c4a2d81f6b05e93c7a4d21f8b6c0e5a39d74
  command: review --watch
  workdir: /workspace
  user: node

  env:
    REVIEW_PROFILE: strict
    NO_COLOR: "1"

  resources:
    cpu: 50%
    memory: 4Gi

  egress:
    http:
      - match: api.some-provider.example
        verdict: allow
        scheme: https
        description: the model API this agent calls
      - match: registry.npmjs.org
        verdict: allow
        binaries:
          - /usr/local/bin/npm
      - match: "*.telemetry.example"
        verdict: deny
      - match: deb.debian.org
        verdict: allow
        description: the mirror this sandbox's pre-start script installs from
    tcp:
      - match: db.example.com:5432
        verdict: allow
        description: the review history it appends to

  credentials:
    - envVar: SOME_TOKEN
      placeholder: some_LNSPLACEHOLDER0000000000
      injections:
        - kind: bearer_header
          domain: api.some-provider.example
    - envVar: REVIEW_DB_PASSWORD
      placeholder: lns-placeholder-review-db
      injections:
        - kind: uri_placeholder
          domain: db.example.com:5432

  tools:
    - node@22
    - python@3.12

  mixins:
    - ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582

  volumes:
    - type: bind
      source: .
      target: /workspace
    - type: volume
      name: reviewer-cache
      target: /home/node/.cache

  filesets:
    - inline:
        REVIEW-RULES.md: |
          Prefer small diffs. Flag any change that adds no test.
      guestPath: /etc/reviewer
    - path: ./prompts
      guestPath: /opt/reviewer/prompts
      owner: root

  ports:
    - container: 8080
      host: 18080

  scripts:
    - when: pre-start
      user: root
      description: the ripgrep this sandbox's prompts assume
      run: |
        apt-get update
        apt-get install -y --no-install-recommends ripgrep
    - when: pre-start
      description: seed the cache this run keeps, in the volume that outlives it
      run: mkdir -p /home/node/.cache/reviewer
```

Read against the rules below, that document is valid: the image is digest-pinned,
`user: node` resolves in the guest's own `passwd`, both `egress` tables carry a
`verdict` on every entry, each credential's `injections` name a destination the
egress reaches, the mirror its `pre-start` script installs from is one the egress
allows, volumes and filesets claim distinct guest paths, and no `envVar`, tool,
guest path, or port repeats.

| Field | Required | Summary |
|---|---|---|
| [`image`](#311-image) | **REQUIRED** | The base OCI image the sandbox runs. |
| [`command`](#312-command-and-workdir) | optional | Replaces the image's default command; keeps its `ENTRYPOINT`. |
| [`workdir`](#312-command-and-workdir) | optional | Absolute guest working directory. |
| [`user`](#313-user) | optional | The user the workload runs as. |
| [`env`](#314-env) | optional | Non-secret environment variables. |
| [`resources`](#315-resources) | optional | vCPUs, memory, and the sandbox's own disk. |
| [`egress`](#316-egress) | optional | Where the workload may reach. |
| [`credentials`](#317-credentials) | optional | The secrets the workload needs and the domains they may reach. |
| [`tools`](#318-tools) | optional | Portable `name@version` toolchain declarations. |
| [`mixins`](#319-mixins) | optional | Mixin references merged into this sandbox at startup. |
| [`volumes`](#3110-volumes) | optional | Named volumes and host binds. |
| [`filesets`](#3111-filesets) | optional | Files shipped inside the artifact, or read off the running machine. |
| [`ports`](#3112-ports) | optional | Container ports the sandbox serves. |
| [`scripts`](#3113-scripts) | optional | Shell scripts the guest runs before the workload starts. |

#### 3.1.1 `image`

```yaml
image: ghcr.io/acme/base@sha256:<64 hex>
```

| Field | Type | Rules |
|---|---|---|
| `image` | string | REQUIRED. MUST NOT be empty or whitespace. Any OCI reference form is accepted. |

An author SHOULD pin the image by digest before publishing. A tag makes the
published sandbox mutable underneath its consumers, which defeats the digest the
consumer approved.

#### 3.1.2 `command` and `workdir`

| Field | Type | Rules |
|---|---|---|
| `command` | string | optional. Replaces the base image's default command. The image's `ENTRYPOINT` still applies. A per-run command after the reference wins. |
| `workdir` | string | optional. MUST be absolute, MUST NOT contain a `..` segment, MUST NOT contain control characters. Created when missing. `-w` / `--workdir` wins per run. |

#### 3.1.3 `user`

The user the workload runs as.

```yaml
user: root          # or node, 1000, node:staff, 1000:20
```

| Field | Type | Rules |
|---|---|---|
| `user` | string | optional. `USER` or `USER:GROUP`. Each segment MUST be non-empty and MUST NOT contain `=`, whitespace, a control character, or a quote. At most one `:`. |

Four sources can name the run-as user, and the first that speaks wins:

```text
-u on the command line  >  spec.user  >  the image's USER  >  the sandbox user
```

**A name and a number mean different things.** A number is used as the uid
directly. A name is left for the **guest** to resolve from its own `passwd`, so
`user: node` runs as whatever uid the image gave `node`. A name absent from the
image's `passwd` refuses the boot rather than falling back to an unintended uid.

Declaring a user is what makes a definition publishable: a workload that needs
`root` to install packages can say so, instead of only working for colleagues who
know to pass `-u root`.

**A declared user is disclosed, never prompted for.** `lns inspect` prints it for
a local definition and for a pulled reference alike, so an artifact that wants
root is visible before it boots. Root is root *inside the microVM*, and reaching
anything of the host's takes a separate bind consent — so there is nothing here
for a prompt to protect.

**Root is bounded, and the bound is what makes that true.** A process running as
root in the guest — the workload, or a [script](#3113-scripts) before it — can
write anywhere the mounts allow and can change ownership, but it cannot
administer the network, create a device node, or set a file capability. The
network capability is the one that matters: without it, nothing inside the guest
can take down the cage that enforces `egress`, so root inside the microVM stays a
statement about the filesystem rather than about policy.

Binding a port is not part of that bound. The guest lowers its own unprivileged
port floor to zero, so any user can bind any port and an image that serves on
`80` runs unchanged as its own unprivileged user. Nothing is exposed by binding:
what leaves the sandbox is decided by [`ports`](#3112-ports), and that binds
loopback on the host.

The quote is excluded for a concrete reason: the value is passed to the guest on
the kernel command line, which is space-joined and honours `"`. A quoted segment
would swallow every key after it.

#### 3.1.4 `env`

```yaml
env:
  IS_SANDBOX: "1"
```

| Field | Type | Rules |
|---|---|---|
| `env` | map<string,string> | optional. Each key MUST be non-empty and MUST NOT contain `=`, whitespace, or control characters. |

`env` is for **non-secret** values. A real secret reaches the workload only as a
credential-shaped placeholder — see [§3.1.7](#317-credentials).

#### 3.1.5 `resources`

A `cpu` or `memory` size is absolute or a **share of the host**. A `disk` size is
absolute only.

```yaml
resources:
  cpu: 80%        # or 4, or "500m"
  memory: 50%     # or 512Mi
  disk: 40Gi
```

| Field | Type | Rules |
|---|---|---|
| `cpu` | int \| string | optional. An integer MUST be ≥ 1. A string is a positive count with an optional suffix (`500m`) or a share (`80%`). |
| `memory` | int \| string | optional. An integer is a MiB count, and MUST be ≥ 1. A string is a byte size (`512Mi`, `2Gi`) or a share (`50%`). |
| `disk` | int \| string | optional. The sandbox's own writable disk. An integer is a MiB count. A string is a byte size (`40Gi`). A share is REFUSED. Default `10Gi`. |

**`disk` — the sandbox's own disk, not a volume.** Everything the workload
writes outside a volume or a bind lands here, and the service discards it when
the run ends. A named volume sizes itself with
[`volumes[].size`](#3110-volumes) and outlives the run.

`disk` takes no share. A share of the host's total cores or RAM names a size the
host can honour, because nothing keeps them spent — a run returns them when it
ends. Disk bytes stay spent. Total disk is already committed — to images, to
volumes, and to the user's own files — so "80% of the disk" names a size the host
does not have. A disk is also sparse, so an author gains nothing by asking for a
share of a machine they cannot see.

| Rule | Detail |
|---|---|
| Minimum | `20Mi`. Near that size the disk runs out of room for its own metadata and a journal the guest can recover, and a disk without that journal does not mount after a crash. The exact boundary follows internal geometry, so the minimum keeps a margin above it. The service REFUSES a smaller size rather than lifting it: unlike a share, an absolute size is a statement, not a request. |
| Ceiling | Less than `16Ti`. The guest filesystem addresses its blocks in 32 bits, and one block is 4 KiB. |
| Sparse | The disk costs what the workload writes, not what the document declares. A large disk is cheap. |

Neither a per-run flag nor a mixin can change `disk`: a mixin carries no
`resources` ([§3.3](#33-kind-mixin)), and there is no `--disk`. The document
that owns the workload owns the size of the disk it writes to.

A share lets one published artifact size itself sensibly on whatever machine
runs it. It applies to `cpu` and `memory` only. Its rules:

| Rule | Detail |
|---|---|
| Range | A whole number, 1–100, with a `%` suffix. `0%`, `101%`, `8 0%`, and `%` are refused. |
| Basis | A percentage of the host's **total** cores and RAM, never of what is momentarily free — so one definition sizes identically on every run of a machine, and `lns inspect` records a size that reproduces. |
| Floor | A share too small to boot is **lifted** to the built-in floor (1 vCPU / 512 MiB). A share is a request for a share, so it MUST NOT produce a guest that cannot boot. |
| Ceiling | A share above the ceiling is **clamped**. An absolute size above the ceiling is still **refused**: any host can answer "some of what you have", but `999999Gi` is a request this host declines. |
| No reading | When the host capacity is unreadable, the share falls back to the default **and** is reported as ignored. The service says so rather than booting a guessed size. |

Per-run `--cpus` / `--mem` win over both forms.

#### 3.1.6 `egress`

An [egress definition](#42-the-egress-definition) — where this workload may reach,
and what it may not.

```yaml
egress:
  http:
    - match: api.example.com
      verdict: allow
      description: the service this agent talks to
  tcp:
    - match: db.example.com:5432
      verdict: allow
```

The table is enforced as written, once the developer has approved the resolved
sandbox ([§1.3](#13-disclosure-before-boot)).

**A destination no entry decides is asked about.** That is the fall-through, and it
is why a verdict is only ever `allow` or `deny`: being asked is the absence of an
entry. To decide every destination instead, end `http` with a catch-all `match`.

#### 3.1.7 `credentials`

A list of [credential definitions](#41-the-credential-definition) — the secrets
this sandbox needs and the domains they may travel to. A sandbox declares as many
as it needs; each one is complete on its own.

```yaml
credentials:
  - envVar: SOME_TOKEN
    placeholder: some_LNSPLACEHOLDER0000000000
    injections:
      - kind: bearer_header
        domain: api.some-provider.example
```

| Rule | Detail |
|---|---|
| Shape | Each entry is a credential definition, with the fields and rules in [§4.1](#41-the-credential-definition). |
| Uniqueness | No two entries may share an `envVar`. |
| Domains | Every `injections[].domain` SHOULD be a destination this document's [`egress`](#316-egress) allows; one it does not is asked about like any other. |

**A declared credential is a requirement.** An author writes one down because the
workload does not work without it, so the run settles every declaration before
boot — by binding a value, or by leaving it to a connector that serves it:

| At boot | Outcome |
|---|---|
| A value is bound on this machine | The workload starts with `placeholder` in `envVar`, and the proxy substitutes the real value on requests to each declared `domain`. |
| No value is bound, and no connector can answer the `envVar` | The run asks the developer for one, naming the `envVar` and the domains it will reach. Answering binds the value and the run continues; declining refuses the run. |
| No value is bound, and a connector this run has not decided claims the `envVar` through one of its credentials | The declaration is **left to that connector**. The workload starts with that connector's `placeholder` in `envVar`, holding no value. The pre-boot disclosure names the connector ([§1.5](#15-one-disclosure)). The first request the connector's `serves` holds raises the ordinary consent card ([§3.2.4](#324-installing-connecting-and-applying)), and granting arms the marker the run already holds. |

**Only a connector that can still answer takes the declaration.** A run that
already **declined** the connector has said its standing no, and a run whose own
document **denies** every destination the connector serves silences the card
([§3.2.4](#324-installing-connecting-and-applying)) — either way no card will
fire, so a marker would leave the requirement unmet in exactly the way declaring
it exists to prevent. Both fall to the row above, and the run asks. A run that
already granted the connector needs no marker where the granted method fills the
variable ([§3.2.4](#324-installing-connecting-and-applying)). A method's
credential may name no `envVar`, or another one
([§4.1](#41-the-credential-definition)) — then nothing answers the declaration,
and the run asks as the first row does.

Asking before boot is the value of writing the credential down: the alternative is
a workload that starts, runs, and fails somewhere inside itself with an unset
variable. Leaving it to a connector answers the same need a different way — the
variable is set from boot, so a client that reads it to decide whether it is
signed in reaches the destination, and the card is what settles it.

**The marker is the connector's `placeholder`, not the declaration's.** Two
sources would otherwise write two markers for one variable, and only one of them
is the one the boundary substitutes for once the method is granted
([§3.3.2](#332-merge-rules) replaces a credential whole). The connector's is
well-defined before any method is chosen: no two installed connectors may claim
one `envVar` ([§7.1](#71-connectors)), and one variable carries one placeholder
across a connector's methods ([§4.1](#41-the-credential-definition)).

**The marker fills the variable only where nothing else set it.** A `-e`, an
`--env-file` entry, the definition's own `env`, and the image's `ENV` each
outrank it. Nothing has consented to the connector, and nothing substitutes for
the marker yet — so a marker that displaced a working value would be an install
breaking a run, which is the one thing [§3.2.4](#324-installing-connecting-and-applying)
forbids. A grant reverses it, as it does for any variable a granted method fills.

**Before the grant the marker registers nothing with the boundary.** It is not a
declared `placeholder` in [§4.1](#41-the-credential-definition)'s sense: no
injection knows it, and nothing substitutes for it. What holds the first request
is the served destination ([§3.2.1](#321-serves)), not the marker. So a marker
sent to a destination the connector does not serve is ordinary bytes, decided by
the run's own rules — harmless, because a placeholder self-identifies as fake by
construction, and the request could not have succeeded either way.

**A declaration is answered before boot, or left to a connector.** Where it is
left, the [connector](#32-kind-connector)'s own card is what answers it: a
connector is offered when a request reaches a destination the run does not deny
([§3.2.4](#324-installing-connecting-and-applying)), which is after boot. The two
paths end in the same mechanism — both end in an injection, and the injection is
identical.

A declaration never carries the value. Per-machine credential values live outside
every document ([§7.1](#71-connectors)).

#### 3.1.8 `tools`

```yaml
tools:
  - node@22
  - python@3.12
  - java@temurin-21.0.5+11.0.LTS
```

| Rule | Detail |
|---|---|
| Shape | Each entry MUST be `name@version`. An entry with no `@` is refused with the version it is missing. |
| Name | MUST match the name pattern. |
| Version | REQUIRED. ASCII alphanumeric plus `.`, `-`, `_`, `+`. MUST NOT be `.` or `..`. `latest` is accepted. |
| Portability | A name is the tool itself, never a provisioner-specific coordinate. A `:` in the name half is an error, because it names a backend this document does not choose. |
| Uniqueness | One entry per tool name. |

Validation is offline. The service provisions declared tools once per machine
before boot, outside workload policy. `lns push` resolves fuzzy versions to an
exact pin — see [§6](#6-publish-time-transforms).

A provisioned tool lands read-only on the workload's `PATH`, **ahead of the base
image's own copies**, so declaring `node@22` decides which `node` the workload
runs even in an image that ships one. It is on the `PATH` a
[`pre-start` script](#3113-scripts) sees too, which is what makes declaring a
tool the alternative to installing one from a script.

#### 3.1.9 `mixins`

```yaml
mixins:
  - ./mixins/postgres-tools/
  - ghcr.io/acme/postgres-tools@sha256:<64 hex>
```

| Field | Type | Rules |
|---|---|---|
| `mixins` | list<string> | optional. A local path or an OCI reference. A remote reference MUST be digest-pinned. |

`lns push` **does not merge** the list. A digest-pinned entry publishes as
written; a local entry publishes the document it names and is rewritten to its
digest ([§6.1](#61-a-local-mixin-publishes-with-the-document-that-names-it)).
Either way each mixin is pulled and merged at startup, and the run presents the
resolved sandbox for approval before it boots ([§1.5](#15-one-disclosure)). A user
can add more for one run with `--mixin`
([§3.3.1](#331-how-a-mixin-enters-a-run)); the merge rules are in
[§3.3.2](#332-merge-rules).

#### 3.1.10 `volumes`

```yaml
volumes:
  - type: bind
    source: /Users/you/dev
    target: /work
    exclude:
      - .cargo
      - .rustup
      - lns-sandboxes
  - type: volume
    name: cache
    target: /home/agent/.cache
    readOnly: false
    size: 100Gi
```

| Field | Type | Rules |
|---|---|---|
| `type` | string | optional. `bind` or `volume`. See the shape table below for the omitted case. |
| `source` | string | Conditional. The host path for a bind; the volume name for `type: volume`. |
| `name` | string | Conditional. The volume name. MUST NOT appear on a bind. |
| `target` | string | REQUIRED. Absolute guest mount path. |
| `readOnly` | bool | optional. Default `false`. |
| `exclude` | list<string> | optional. Bind only. Subpaths of `source` the guest MUST NOT see. |
| `optional` | bool | optional. Default `false`. Bind only. A `source` the running machine does not have is skipped instead of refusing the run. |
| `size` | int \| string | optional. Named volume only; REJECTED on a bind. An integer is a MiB count. A string is a byte size (`100Gi`). A share is REFUSED. Default `10Gi`. |

**`size` — a floor, not a fixed size.** A named volume outlives the run that
made it, so `size` states the capacity the volume MUST have before the run
starts, not the capacity it was created with:

| The volume | What the service does |
|---|---|
| Does not exist | Creates it at `size`. |
| Is smaller than `size` | Grows it in place to `size`, and keeps its contents. |
| Is `size` or larger | Nothing. The floor is already met. |

The service never shrinks a volume. A shrink must choose which data to drop, and
no document may make that choice on a machine it cannot see. Lowering the number
is therefore not an error and not an instruction — the volume already satisfies
the smaller floor. A developer who wants the space back removes the volume
(`lns volume rm`) and lets the next run create it at the declared size.

This is why the floor is a floor and not an equality. `size` defaults to `10Gi`,
so a document that never mentions a size still declares one. If a smaller
declaration refused the run, every such document would refuse every volume that
had ever grown.

One volume name may be mounted at more than one `target`, in one document or
across merged sources, and each entry may declare its own `size`. **The volume's
floor is the largest `size` any surviving entry declares.** Each entry states
what that mount needs; the volume satisfies all of them at once.

`size` takes no share, for the same reason [`resources.disk`](#315-resources)
does not, and its minimum (`20Mi`) and ceiling (less than `16Ti`) are the same.
A volume is sparse: it costs what the workload wrote, not what the document
declared.

The three legal shapes:

| `type` | Source field | Rules |
|---|---|---|
| `bind` | `source` REQUIRED | `name` is REJECTED. `source` MUST be non-empty, MUST NOT contain whitespace, `"`, or control characters, and MUST NOT contain a `..` segment. A relative source resolves against the consumer's own project directory, not the publisher's. |
| `volume` | exactly one of `source` or `name` | Setting both is an error. |
| omitted | `name` REQUIRED | `source` is REJECTED — a source needs an explicit `type`. |

A volume name MUST be non-empty, MUST NOT be `.` or `..`, and MUST use only
ASCII alphanumerics, `_`, `.`, and `-`.

**`exclude` — binding a directory without everything in it.** Binding a parent
directory is often the only sensible mount and also brings things the workload has
no business reading: another sandbox's state, a host toolchain the guest must not
use, an unrelated project. Each entry names a path relative to `source`, and the
guest gets an empty mount there instead of the real contents — the path exists, and
what was behind it does not. Where a [fileset](#3111-filesets) writes to that path,
the guest gets a plain directory holding the fileset's files instead of an empty
mount.

| Rule | Detail |
|---|---|
| Relative | Each entry is relative to `source`. Every segment MUST be non-empty and MUST NOT be `.` or `..`, which refuses a leading `/`, a trailing `/`, and `a//b` in one rule. |
| Exact | An entry names one path, not a pattern. There are no globs. |
| Absent is fine | An entry naming a path that is not there does nothing. |
| Author's rule, not a decision | An exclusion is part of the document, so it is never prompted for and never recorded as a per-machine choice. |

An exclusion travels with the sandbox: a published bind masks what its author
meant, on whatever machine runs it.

What the mask leaves is guest-local, so a [fileset](#3111-filesets) may write
there. [§3.1.11](#3111-filesets) states that rule, and how a bind with such an
exclusion is mounted.

A developer who wants to mask something the author did not name writes a mixin of
their own with that `exclude`, and layers it with `--mixin` or from `spec.mixins`.
A mixin the user names merges after a pulled document, so the pulled one cannot
undo it ([§3.3.2](#332-merge-rules)).

**`optional` — a source not every machine has.** A published sandbox names host
paths that exist on the author's machine and not on every consumer's. `optional:
true` says the mount is a convenience, not a requirement: the run skips it and
continues. The default is `false`, so a missing source refuses the run and the
developer learns why.

`optional` applies to a **bind only**. A named volume is created on demand, so it
is never absent, and setting `optional` on one is an error. The same field
appears on a [`hostPath` fileset](#3111-filesets) and means the same thing.

`target` rules, shared with [`filesets`](#3111-filesets):

- MUST be absolute, MUST NOT contain a `..` segment, MUST NOT contain control
  characters.
- MUST NOT overlap the `/.lens` runtime namespace, which belongs to the sandbox
  itself. `/` is likewise refused.
- MUST be unique across **all** `volumes` and `filesets` in the document.
  Uniqueness is decided on path segments, not on the string, so `/home/node` and
  `/home/node/` are one claim, not two.
- A fileset `guestPath` and a volume `target` whose paths **nest** — `/home/node`
  and `/home/node/.config` — are two claims the mount order decides between;
  [§3.1.11](#3111-filesets) says which of those a document may make. Two
  `volumes` that nest, and two `filesets` that nest, are both allowed: the guest
  mounts nested volumes in declaration order, and two filesets writing under one
  another are just files.

#### 3.1.11 `filesets`

Files the guest gets: shipped by the document, or read off the machine that
runs it.

```yaml
filesets:
  - inline:
      config.json: '{"mode":"review"}'
    guestPath: /home/agent/.agent
  - path: ./skills
    guestPath: /home/agent/.agent/skills
    owner: root
  - hostPath: ~/.gitconfig
    guestPath: /home/agent/.gitconfig
    optional: true
```

| Field | Type | Rules |
|---|---|---|
| `path` | string | Conditional. A directory beside this document, packed at publish. Non-empty. |
| `inline` | map<string,string> | Conditional. Relative path → file content. MUST NOT be empty. |
| `hostPath` | string | Conditional. One file on the machine that runs the document, read at launch and never packed at publish ([§6](#6-publish-time-transforms)). See the rules below. |
| `guestPath` | string | REQUIRED. Where the files land in the guest. Same rules as a volume `target` ([§3.1.10](#3110-volumes)), except that it MAY be home-anchored — see below. A `hostPath` entry carries one file, so its `guestPath` MUST NOT end in `/`. |
| `owner` | string | optional. `workload` (default) or `root`. |
| `optional` | bool | optional. Default `false`. `hostPath` only. A file the running machine does not have is skipped instead of refusing the run. |

**Exactly one** of `path`, `inline`, or `hostPath` MUST be set. None or more than
one is an error. `optional` on a `path` or `inline` fileset is an error too:
those always ship.

**`guestPath`, because a fileset is not a mount.** A volume `target`
([§3.1.10](#3110-volumes)) is a mount point: the guest mounts a host directory or
a named volume there. A fileset is not. The run writes its files into the guest's
filesystem before the workload starts. Nothing is mounted at the path, and
nothing can be unmounted from it. The field says where the files land, which is
what `guestPath` names. The two keep separate words because they are separate
things — one attaches a filesystem, the other places files.

**A `guestPath` nested under a volume `target`.** The mounts happen after the
files are placed, so a `guestPath` under a volume `target` names a path the mount
covers. The run does not leave the file where the mount hides it: it writes the
file into the volume once the volume is mounted, and it does so on **every** boot,
because a fileset is re-derived from its source every run. What the workload wrote
there last run does not survive — the document is what decides that path.

A fileset writes exact paths, and those paths are what the rules below compare. An
`inline` fileset writes `guestPath` joined with each key, so `guestPath: /root`
carrying a `.claude.json` key claims `/root/.claude.json` and nothing else. A
`hostPath` fileset carries one file and claims its `guestPath`. A `path` fileset
places a whole directory, so it claims the whole `guestPath` directory.

Two mounts take no write from a fileset, and a document whose claim lands under
one is **refused**:

- a **bind**, because the write would create a file in the host directory the bind
  shares, and it would outlive the run. A fileset never writes to the host
  filesystem.
- a **read-only** volume, because it takes no write at all, so the file could
  never land.

The refusal names the fileset entry and the volume entry. The nesting runs either
way — a claim under a `target`, or a `target` under a claim — and is refused the
same.

A bind's `exclude` ([§3.1.10](#3110-volumes)) is the one place inside a bind that
does take the write. The mask it leaves is guest-local, so a file written there
never reaches the host and does not outlive the run. A claim under a bind is
allowed when an `exclude` entry of that same bind covers it. Another bind's
`exclude` does not, and `readOnly` makes no difference, because the mask is
guest-local whatever the bind does with the share.

A bind with such an `exclude` is mounted entry by entry rather than whole, because
the masked path needs a guest-local parent for the write to land there. The split
follows the excluded path: the bind `target`, and every directory between it and
the masked path, is a guest-local directory, and the host entries beside them are
mounted one at a time. At each of those levels every entry is its own mount point:

- A rename over one, or an unlink of one, fails.
- An entry the workload creates beside them is guest-local, and it does not reach
  the host.
- An entry that appears on the host after the run starts does not show up.

Paths inside those entries behave as any bound path does. A bind whose `exclude`
no fileset covers is mounted whole.

A document validates against its own `volumes`, so a run that adds one of those
two mounts itself — a read-only or bind mount the launch names rather than the
document — is refused when the run starts instead, before anything boots.

A `path` or `inline` fileset is **not a separate artifact**. `inline` content
lives in this document, and a `path` directory is packed into a layer of the same
artifact this document configures, so the files and the declaration that places
them share one digest and are approved together. To share one directory across
several sandboxes, publish a [mixin](#33-kind-mixin) that carries it.

`owner: workload` lets the workload rewrite its own seeded state.
`owner: root` pins inputs the workload MUST NOT touch.

**`hostPath` — seeding the guest from the machine.** An agent wants the
developer's `~/.gitconfig`, and the alternative to naming it is a bind of the
whole home directory, which is worse. A `hostPath` is the narrow form: one file,
**snapshotted at launch** and never packed, so the guest gets a copy. With
`owner: workload` the workload may rewrite that copy, and the host file does not
change. This is what separates it from a bind ([§3.1.10](#3110-volumes)), which
is live and two-way.

`hostPath` rules, all enforced offline:

| Rule | Detail |
|---|---|
| Anchored | MUST start with `/` or `~/`. `~/` is the running developer's home. Another user's home (`~alice/`) is refused — a document does not choose whose files it reads. |
| Contained | MUST NOT contain a `..` segment. |
| Literal | MUST be free of whitespace, quotes, and control characters. |
| Not secret-shaped | The same name check as an inline path, below. A `hostPath` file is copied whole, so its name is the only guard it has. |
| One file | It names a file, not a directory, so `guestPath` MUST NOT end in `/`. |

A home-anchored `path` is refused and points here: `path` packs a directory
beside the document at publish, and the publisher's home is not the consumer's.

Inline limits, all enforced offline:

| Limit | Value |
|---|---|
| Files per inline fileset | 256 |
| Bytes per file | 131072 (128 KiB) |
| Bytes per fileset | 1048576 (1 MiB) |
| Path length | 4096 bytes |

An inline path MUST be a safe relative path: non-empty, no leading `/`, no
empty, `.`, or `..` segment, and no control characters.

**Secret-shaped names are refused.** Real secrets stay outside the workload, so
a path segment naming one fails the document — in a `path` directory, in an
`inline` key, and in a `hostPath`. The check covers
names starting with `.env`, `credentials.`, or `.credentials`, names ending in
`.pem`, `.key`, `.ppk`, or `.keystore`, and the exact names `.npmrc`, `.netrc`,
`.git-credentials`, `.pgpass`, `.pypirc`, `.yarnrc.yml`, `auth.json`,
`credentials`, the SSH key names (`id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519`),
and the credential directories `.ssh`, `.aws`, `.gnupg`, `.kube`, `.azure`,
`.oci`, `.docker`.

Both spellings of `credentials` are named because a per-user store is usually the
dotted one. `.credentials.json` is the file a coding agent keeps its subscription
token in, and a check that reads only `credentials.` lets that name through.

A [connector](#32-kind-connector) is the one exception, and a narrow one. Its
filesets exist to write exactly these files, and
[§3.2.5](#325-a-fileset-carries-the-placeholder-not-the-value) requires every one
of them to carry a placeholder rather than a value. So a connector may use a
secret-shaped name, and a connector fileset that uses one and declares no
placeholder in it is refused.

**`guestPath` MAY be home-anchored.** A `guestPath` starting `~/` resolves
against the home directory of the run's [`user`](#313-user), read from the guest.
It does **not** vary with `owner`: `owner: root` decides who owns the file, never
where it lands. Another user's home (`~alice/`) is refused, as it is for a
[`hostPath`](#3111-filesets); the two `~/` are otherwise unrelated, one being the
guest's home and the other the developer's. The form exists because a client's own
documentation names the file it wants as `~/.claude/.credentials.json`, and
`/home/agent` is wrong on any image whose user is named something else.

A [`user`](#313-user) given as `uid:gid` has a home only if the image's passwd
names one. Where it does not, a `~/`-anchored `guestPath` **refuses the run**,
naming the fileset and the `user` it could not resolve.

**Home resolution happens at launch, so path uniqueness splits in two.** Offline
validation compares the paths as written and refuses two that are literally the
same; it cannot see that `~/.agent` and `/home/agent/.agent` are one location. That
collision is caught at launch ([§5](#5-validation-summary)), and the run refuses,
naming both entries and the spelling each used.

[§3.3.2](#332-merge-rules) keys the fileset union on the path **as written**, so a
mixin writing `/home/agent/.claude` does not override a connector method's
`~/.claude` — both survive the merge and the run refuses for the collision above.
To override an entry, match its spelling.

**A pulled `hostPath` is a per-machine decision.** A `hostPath` makes what a
document reads depend on the machine running it, so an artifact from a registry
MUST NOT read one on the strength of its own declaration. On the first run that
would read it, the developer is asked, and the answer is recorded per machine —
keyed by the artifact's repository and the host path, so a version bump does not
ask again and a different sandbox does not inherit the answer. A declined
`hostPath` skips the fileset when it is `optional` and refuses the run when it is
not. A document loaded from the developer's own directory is their own consent
and is never asked about.

The decision is per machine, so it lives beside the other per-machine state and
not in the [run's decisions](#8-the-runs-decisions): that file is one run's
answers, and this is a risk accepted by one developer on one computer.

#### 3.1.12 `ports`

```yaml
ports:
  - container: 8080
    host: 18080
```

| Field | Type | Rules |
|---|---|---|
| `container` | int | REQUIRED. 1–65535. MUST be unique within the document. |
| `host` | int | optional. 1–65535. MUST be unique within the document. |

A declared port is published when the run starts — compose-style, and the same
way whether the sandbox is a local `./lns.yaml` or a pulled reference. The
mapping appears in the resolved sandbox the developer approves
([§1.5](#15-one-disclosure)), so there is nothing a second opt-in would tell them
that approving the run did not.

Two properties bound what publishing can do:

- **Loopback only.** A published port binds `127.0.0.1`, never a routable
  address, so a sandbox cannot expose a service to the network the host sits on.
- **`host` is a request.** A host port already in use refuses the run and says
  which port collided, rather than silently picking another — the number is how
  the developer reaches the service. Omit `host` to let the run report the port
  it chose.

#### 3.1.13 `scripts`

Shell scripts the guest runs before the workload starts, each under a user it
names. This is where a document configures what it contributes: a package the
image lacks, a generated config file, a seeded cache.

```yaml
scripts:
  - when: pre-start
    user: root
    description: the psql this sandbox's prompts assume
    run: |
      apt-get update
      apt-get install -y --no-install-recommends postgresql-client
  - when: pre-start
    run: psql --version > /tmp/psql-version
```

| Field | Type | Rules |
|---|---|---|
| `when` | string | REQUIRED. The slot the script runs in. `pre-start` is the only slot this grammar defines; any other value is refused. |
| `run` | string | REQUIRED. The script body, run by `sh -e`. MUST be non-empty and MUST NOT contain a NUL, which would truncate the file the shell reads. At most 128 KiB. |
| `user` | string | optional. `USER` or `USER:GROUP`, the same grammar and the same guest resolution as [`user`](#313-user). Absent means the user the workload itself runs as. |
| `description` | string | optional. What this script is for, shown in the disclosure the way an [`egress`](#42-the-egress-definition) rule's own `description` is. |

A document declares at most **32** scripts totalling at most **512 KiB** of `run`
bodies. Each script becomes a file the run stages inside the guest, so the ceiling
is stated here rather than discovered at boot. A script longer than that belongs
in a [fileset](#3111-filesets) the document ships, with a short script to run it.
The ceiling bounds **one document**: resolution appends across sources
([§3.3.2](#332-merge-rules)), and the sum is nobody's authoring mistake to
correct.

A script has no name and no id. Entries are positional and they append rather
than override, so there is no key for anything to merge on.

**`when` exists so a later slot is not a format change.** One slot answers the
need this block was added for, and naming it in every entry means a cleanup slot
arrives as one more accepted value rather than as a second block.

**`pre-start` is the last thing before the workload**, so everything else the
sandbox promised is already in place when a script runs:

| By the time a `pre-start` script runs | So a script can |
|---|---|
| The rootfs is composed, and `volumes`, binds, and `filesets` are mounted and materialized. | Read a fileset's files, and write into a volume. |
| Declared [`tools`](#318-tools) are on `PATH` ahead of the image's own copies, as [§3.1.8](#318-tools) states. | Use a declared toolchain rather than installing one. |
| The network cage is up and the run's `egress` is in force, with the proxy and the credential injection ([§4.1](#41-the-credential-definition)) behind it. | Reach exactly what the workload may reach, and no more. |
| `env` is set, and each declared credential's `envVar` holds its placeholder. | Authenticate to a destination its `injections` name. |
| The workload has not started. | Fail the run before anything the workload does can depend on it. |

The script runs with `sh` from the guest's own filesystem, in the home directory
of the user it runs as — `/` when that user has none. An image that ships no `sh`
cannot run a script, which is a launch failure and not an authoring one: no
offline check can see inside the image.

**The body runs under `-e`, so the first command that fails ends the script.** A
multi-line body is a sequence the author expects to complete, not a best-effort
list: an `apt-get update` that fails followed by an `apt-get install` that
succeeds off a stale cache would otherwise exit `0` and hand the workload exactly
the half-prepared environment this block exists to prevent. An author who wants a
command's failure tolerated says so where it is — `cmd || true`. Output streams to the run's own output as the
script produces it.

**A script has no stdin.** It reads `/dev/null`, so a tool that would ask a
question reads an EOF instead of waiting. Nothing else would be answerable: the
run's own console belongs to the workload that has not started, and a script is
not attached to anyone. A script is not bounded by a timeout either, as this
section states below, so an inherited stdin would hold a boot open forever with
nothing to show for it.

**A document that ships `apt-get install` ships the egress for its mirror too**,
in the same document, because a script is not a way around the policy the
consumer approved. A destination no rule decides is asked about the way any other
is — before the workload has started.

**A script's writes are as durable as where it writes.** Everything outside a
volume or a bind lands on the sandbox's own disk
([§3.1.5](#315-resources)), which the service discards when the run ends — so an
installed package is installed again on the next boot. A write into a `volume`
outlives the run, and a write into a read-write bind reaches the host, exactly as
the same write from the workload would. This is the difference from
[`tools`](#318-tools), which the service provisions once per machine and caches:
a tool is portable and installed outside workload policy, while a script is
specific to this image and re-done per run. Declare a tool where a tool fits.

**A script running as root is bounded the way the workload's root is** — the same
capability set, described in [§3.1.3](#313-user) — so a script cannot dismantle
the cage it is about to hand the workload. That holds however the script arrived
at root: by naming `user: root`, or by naming no user in a run that `-u root`
promoted. A package whose installer sets a file capability or creates a device
node fails partway through, so a script that installs one reports a failure
rather than a working package.

**A non-zero exit refuses the run.** Later scripts do not run, and the workload
never starts. This is a failure before the workload started, so `lns run` exits
`125`, identifying the script the way the disclosure does — its position in the
run order and the document that contributed it — along with the status it exited
with. The workload's own status codes stay reserved for the workload
([`docs/cli-spec.md`](cli-spec.md) §5). A script is part of the environment the
resolved sandbox promised; starting the workload without it would turn one clear
failure into several confusing ones — the same reason a `user` the guest cannot
resolve refuses the boot rather than falling back ([§3.1.3](#313-user)).

**A script that names no user follows the effective run-as user**, not
`spec.user` — so `-u root` promotes every such script to root, exactly as it
promotes the workload.

**A script is not bounded by a timeout.** A slow install and a hung one look the
same from outside, and cutting one off would fail runs that were about to
succeed; a script that waits forever holds the run the way a workload that waits
forever does. Its output streams as it is produced, so a reader can tell the two
apart, and `lns stop` ends the run.

Script output is not recorded in the audit trail as output — it reaches the run's
own output, which `lns logs` serves. What the run **records** is that each script
ran: its position, the document that contributed it, the user it ran as, and the
status it exited with, under the `launch` kind.

---

### 3.2 `kind: connector`

A connector is a [mixin](#33-kind-mixin) the user installs on their own machine.
No document names it, and it is **offered** rather than resolved.

Two blocks, with two different jobs:

| Block | Job |
|---|---|
| `serves` | When is this connector worth offering. Grants nothing. |
| `methods` | The alternative ways to connect, each carrying what applies once it succeeds. |

```yaml
apiVersion: lns.run/v1
kind: connector
name: some-provider
spec:
  serves:                                    # detection only
    - api.some-provider.example

  methods:
    - name: token
      label: API token
      auth:
        kind: token
        help: Create one at some-provider.example/settings/tokens
      egress:
        http:
          - match: api.some-provider.example
            verdict: allow
      credentials:
        - envVar: SOME_TOKEN
          placeholder: some_LNSPLACEHOLDER0000000000
          injections:
            - kind: bearer_header
              domain: api.some-provider.example
      env:
        SOME_PROVIDER_REGION: eu
```

**Installing grants nothing** — no destination opens, no injection is armed, no
file is written, and no real value is supplied. What it does do is make its
`serves` destinations **ask** ([§3.2.1](#321-serves)), and lend its `placeholder`
to a variable a sandbox itself declared and left to it
([§3.1.7](#317-credentials)).

Two later acts change what a run gets: a **grant** applies one method to one
run, and a **connect** authenticates where a method requires it
([§3.2.4](#324-installing-connecting-and-applying)).

Three stored facts, each about a different scope:

| State | Means | Scope |
|---|---|---|
| **installed** | the document is on this machine, so it can be offered | the machine |
| **connected** | at least one **connection** exists — one successful connect, holding what its `auth` produced | the machine |
| **granted** | this run may use a named method, and the connection behind it where that method authenticates | the run, or the name reserved for one |

A run has a granted method's payload **applied**: in force in the guest. That is
not a fourth stored fact, only what a run does with the three above.

#### 3.2.1 `serves`

The destinations that make this connector relevant, written in the
[`match`](#42-the-egress-definition) grammar. At least one entry.

**`serves` grants nothing — it asks.** While a run has neither granted nor
declined this connector, every destination `serves` covers is held and asked
about, **whatever the run's own [egress](#42-the-egress-definition) allowed** — a
request that reaches a service unauthenticated fails just as surely as one that is
blocked.

A destination the run **denies** does not ask: a deny is an explicit refusal.

What opens on a grant is the granted method's `egress`, which MAY be narrower than
`serves` and MAY name destinations `serves` never mentions — a sign-in host, for
instance. So write `serves` broadly: a destination it misses is one the connector
is never offered for.

**An entry with no port matches any port.** A port narrows it
(`db.some-provider.example:5432`). This is the opposite default from
[`egress.tcp`](#42-the-egress-definition), where a port is REQUIRED, because a
bare host matching only the HTTP ports would leave every raw-stream connector
installed and never offered.

`serves` is one list, not an `http` and a `tcp` table: a connector serves a host
however the request arrived, and the transport decision belongs to the method's
`egress`. So a raw-stream service needs no extra syntax:

```yaml
spec:
  serves:
    - db.some-provider.example                 # any port
  methods:
    - name: password
      auth:
        kind: token
      egress:
        tcp:
          - match: db.some-provider.example:5432
            verdict: allow
      credentials:
        - envVar: PGPASSWORD
          placeholder: some_LNSPLACEHOLDER0000000000
          injections:
            - kind: uri_placeholder
              domain: db.some-provider.example:5432
```

`uri_placeholder` is the injection family that reaches a raw stream
([§4.1](#41-the-credential-definition)): the workload sends the placeholder as
its password and the proxy swaps it in transit.

**One connector per destination.** Two installed connectors whose `serves`
overlap make the offer ambiguous, so the second install is refused
([§7.1](#71-connectors)). Overlap is computed on the patterns with their ports,
so two connectors on different ports of one host coexist; two on the same host
and port do not, whatever transport each method declares.

#### 3.2.2 `methods`

The ways to connect, in the author's order of preference. At least one.

**Methods are alternatives, never a set.** One is chosen, and exactly one
method's payload applies. A service needing two secrets at once is one method
producing two values.

| Field | Type | Rules |
|---|---|---|
| `name` | string | REQUIRED. A DNS label ([§2](#2-common-top-level-fields)), unique within the document. The stored identity: what a connection records as the method that produced it, and what `lns connector connect --method` names. |
| `label` | string | optional. Display text for the card; defaults to `name`. It MAY be rewritten between versions without signing the machine out, because a connection records the method's `name` ([§7.1](#71-connectors)). |
| `auth` | map | optional. How the user proves they may use this method. Absent means there is nothing to prove. |
| [`egress`](#42-the-egress-definition) | map | optional. Opened when this method applies. |
| [`credentials`](#41-the-credential-definition) | list | optional. Armed when this method applies. |
| [`filesets`](#3111-filesets) | list | optional. Written when this method applies. |
| [`env`](#314-env) | map | optional. Set for workloads that start after the grant ([§3.2.4](#324-installing-connecting-and-applying)). |

**The payload lives in the method and nowhere else.** No block a method may carry
may also appear under `spec`. A connector whose methods share a payload repeats
it.

A method with no `auth` is not a lesser method — a connector that only opens
`docs.rs` has nothing to sign in to. It is never connected, only granted:

```yaml
spec:
  serves: [docs.rs]
  methods:
    - name: default
      egress:
        http:
          - match: docs.rs
            verdict: allow
```

**`auth`.**

| Field | Type | Rules |
|---|---|---|
| `kind` | string | REQUIRED. The mechanism. Decides what else `auth` accepts and what values the method produces. |
| `label` | string | optional. Names this mechanism on the card when a connector offers several. |
| `help` | string | optional. Plain text the card shows: where to get a value, what to expect. |

| `kind` | Produces | Notes |
|---|---|---|
| `token` | `token` | The user supplies one value. |

A method's `credentials` draw from what its `auth` produced
([§4.1](#41-the-credential-definition) `field`), so a method declaring
`credentials` and no `auth` is **refused** — the credential would ship
permanently unarmed.

**An unknown `auth.kind` MUST NOT refuse the document**, and this is a stated
exception to [§1.2](#12-strict-decoding). A reader that does not know a `kind`
does not decode that `auth` at all: it marks the method unofferable, offers every
other method, and the card names the method that needed a newer `lns`.

The exception is bounded to that one map. It tolerates unrecognized keys directly
under an `auth` whose `kind` the reader does not know, and nothing further — not
the method's own fields, and not the body of an `auth` whose `kind` the reader
does know.

#### 3.2.3 What a method may carry

A method is applied to a guest that is **already running**, so it may declare
only what such a guest can still be given. Every other block is refused wherever
the document is parsed — at `lns artifact validate`, at push, and at install,
which is where a user meets the refusal.

| Block | Method | Why |
|---|---|---|
| [`egress`](#42-the-egress-definition) | MAY | The gate reads a fresh table on every policy change. |
| [`credentials`](#41-the-credential-definition) | MAY | Injection is domain-keyed and re-read per policy change, so a grant arms a value mid-run. |
| [`filesets`](#3111-filesets) | MAY — `inline` or `path` | Written into the running guest at the path the entry names. |
| [`env`](#314-env) | MAY | Applies to workloads that start afterwards, which is the only moment a variable can be given. |
| `tools` | MUST NOT | Provisioned into the image layer before boot, and contributes to `PATH`. |
| `scripts` | MUST NOT | Run once, before the workload. Connecting a connector MUST NOT mean running code. |
| `volumes`, `ports` | MUST NOT | Which mounts and which listeners exist is fixed when the guest is created. |
| `mixins` | MUST NOT | The card shows one document. A graph would apply egress and credentials from documents the user never saw. |
| `image`, `command`, `workdir`, `user`, `resources` | MUST NOT | The [mixin](#33-kind-mixin) rule, for the same reasons. |

A `filesets[].hostPath` is refused too: a connector is installed once and used in
every project, so reading a path off whichever machine happens to be running it
is a [sandbox](#3111-filesets) concern.

The refusal names the mechanism rather than the rule, so an author learns where
the block belongs instead:

```text
a connector must not declare tools: a connector is applied to a guest that is
already running, and tools are installed before the guest boots
```

**A connector fileset's `guestPath` MUST be `~/`-anchored.** A
[sandbox](#3111-filesets) fileset is built into the image layer before boot and
may land anywhere. A method is applied on a policy change, and that reaches only
the home of the guest that is already running. An absolute path is refused
wherever the document is parsed, so a document that installs is a document that
can be delivered — the rule holds offline what would otherwise be discovered at
grant.

One consequence, and it is the intended one: a run whose
[`user`](#313-user) is a `uid:gid` the image's passwd does not name has no home,
so it cannot take a method that writes a fileset. It refuses the same way
[§3.1.11](#3111-filesets) refuses a `~/`-anchored sandbox fileset.

**A method's filesets MUST NOT exceed 1 MiB in total**, the same ceiling
[§3.1.11](#3111-filesets) sets on one inline fileset. The limit is per method,
not per document, because methods are alternatives and only one is ever granted.
It is a running cost rather than a one-time one: the files are re-sent on every
policy change, and all granted connectors share one budget.

`inline` bytes are counted offline. A `path` a validator cannot read — a pulled
artifact's packed layer — is counted at push, where the layer is built, and at
install, where the layer is kept.

**An `inline` file lands mode `0600`**, which suits the credentials file
[§3.2.5](#325-a-fileset-carries-the-placeholder-not-the-value) exists to write.
A `path` file keeps the mode it had when the directory was packed. A connector
fileset entry declares no mode of its own.

#### 3.2.4 Installing, connecting, and applying

**Installing grants nothing**, and an installed connector contributes no merge
source ([§3.3.2](#332-merge-rules)). It makes its destinations ask
([§3.2.1](#321-serves)). The one thing it puts in a running guest is a marker in
a variable a sandbox itself declared and left to it
([§3.1.7](#317-credentials)) — the declaration's doing, not the install's.

**Granting has three parts, in this order:**

1. **Choose a method**, and where it authenticates, a connection — one this machine
   already holds, or a new one. The card offers each, labelled with what applying
   it will do: the destinations it opens, the variables it sets, the files it
   writes.
2. **Satisfy the method's `auth`**, where it has one and a new connection is being
   made. This happens on the host, outside the guest, and produces the values the
   method's `credentials` draw from. A method with no `auth`, and an existing
   connection, both skip this part.
3. **Apply the method.** Its `egress`, `credentials`, and `filesets` reach the
   running guest, and a grant is recorded for this run — the method, and the
   connection behind it where one exists
   ([§8.6](#86-where-a-connector-grant-goes)). Where a marker mismatch defers the
   grant, the record is written and the payload reaches the guest at the run's
   next start instead (see below).

Authentication that fails or is abandoned grants nothing and leaves the offer
standing.

**Two prompts, two triggers:**

| Prompt | Asks | Fires on |
|---|---|---|
| The **consent** card | choose a method — and a connection, where it authenticates — and use it in this run | a destination `serves` covers that the run's own egress does not **deny** |
| The **connect** prompt | re-authenticate the connection this run granted | a request held because it carries an unarmed `placeholder` ([§4.1](#41-the-credential-definition)) |

A grant MAY also be given ahead of any request, with the same disclosure and no
held request (`lns connector grant`). The trigger decides when the card
appears, never whether consent must be informed.

**A grant belongs to one run.** Not to a directory: the working directory
roots the paths a reference resolves against, and nothing about consent
([§8.5](#85-what-the-working-directory-decides)). A run keeps what it was granted
across every stop and start. It loses that grant when the run is removed, and not
before.

**A grant MAY also be given ahead of the run**, to a name no run holds yet. It
is then a **reservation**. Exactly one act takes a reservation: **creating a run
with an explicit name**. Two rules follow, and together they stop a reservation
becoming a standing consent that any later run inherits:

1. **A reservation is consumed once.** The run created with that name takes it,
   holds a grant of its own from then on, and the reservation is gone. A second
   run of that name is asked.
2. **An automatically generated name never takes one.** A reservation records
   the user naming a run, and the user did not name that one. The generator does
   not choose a reserved name either, so the two rarely meet. Where they do —
   a reservation written in the same instant a run is given that name — the
   reservation is **discarded**, because the name now belongs to a run that did
   not consent to it.

**Renaming a run takes nothing.** A rename moves a handle; it does not create a
run, and a reservation is for a run that does not exist yet. So renaming a run
onto a reserved name leaves both the run and the reservation as they were.

**A run takes its reservation when it is created**, before the resolved sandbox
is disclosed ([§1.5](#15-one-disclosure)). So the first run of that name
discloses the granted method like any other, and nothing reaches a guest that
the pre-boot disclosure did not name.

**A `deny` is the only verdict that silences the card.** Everything else asks —
a destination nothing decided, and a destination the run allows. So a run holds a
request for a served destination it would otherwise have allowed, until this
run grants or declines the connector. A connector is not offered before boot
either: consent is given against a live need, and before boot there is no request
to consent for. Where a declaration named it ([§3.1.7](#317-credentials)), the
pre-boot disclosure has already said the connector will be asked about.

The card is what a `deny` silences, not a grant. `lns connector grant` gives a
grant ahead of any request, and it is not refused because some run's document
denies the destination — the developer is choosing, not being prompted to
overturn.

**A granted method can still override a document's `deny`.** A grant outlives
the verdict that stood when it was given, and `lns connector grant` may be given
over a live deny, so a granted method keeps applying whether the deny arrived
before or after it — a connector is source 4, and
[§4.2](#42-the-egress-definition) lets a later source turn a `deny` into an
`allow`. The override MUST be named in the pre-boot
disclosure ([§1.5](#15-one-disclosure)) and on the consent card that introduces
it. A reservation is no exception: a run takes one as it is created, before the
disclosure resolves. The [run's decisions](#8-the-runs-decisions) are unaffected:
they are source 5, so a `deny` the developer answered still wins.

**The held request waits, and applying does not depend on it.** The request that
triggered the offer stays held while the user chooses a connection and satisfies any
`auth`; when the connection applies, the hold is released and the request proceeds.
The deadline is the workload's, not the run's, and a mechanism that sends the user
to a browser can outlast it — then that request fails as any refused request does.
**An expired hold does not cancel the connect:** the user finishes, the connection
applies, and the next request succeeds. A deferred grant is the one case where
applying does not follow: the hold is released without the request proceeding, and
the next request succeeds only after the run's next start (see below).

**`env` reaches the next workload of that run.** The granted method's `env`, and
the `placeholder` of each credential it declares, are set for workloads that
start after the grant; the workload that triggered the offer keeps the
environment it was launched with. A tool authenticating over the wire works
immediately; one reading a variable the grant introduces works from the next
`lns sandbox exec`, or the next start of that run. It does **not** reach a fresh
`lns run`: that is a different run, and it holds no grant.

**A variable the declaration already filled is the exception, and it works at
once.** Where the run declared the credential, left it to this connector
([§3.1.7](#317-credentials)), and the marker filled the variable, every live
session already holds that marker — and the grant arms the marker the run booted
with. Nothing has to change in the environment, so a tool reading the variable
works in the session that raised the card, with no exec and no restart. That is
what leaving a declaration to a connector buys.

**A grant arms the marker the run booted with, or it waits for the next start.**
This decides one case only: a run that booted with a marker for a variable the
granted credential claims. The installed set can change in between — a connector
may be republished under a new `placeholder`, or uninstalled and replaced by
another claiming the same `envVar` — and then the grant would arm a marker no
live session holds. So a grant whose `placeholder` differs from the marker the
run booted with **takes effect at that run's next start**: the grant is recorded,
no payload applies, and the card says so. The request that raised it fails as any
refused request does. A grant whose placeholder matches applies at once, and so
does every grant to a run that booted with no marker for that variable — the
ordinary case, and nothing here changes it.

The rule is uniform because the alternative is worse than a failed request. A
stale marker in a header the proxy sets is overwritten and the request succeeds
by accident; a stale marker a `uri_placeholder` injection was meant to replace
travels as fake bytes and the request fails. A rule that held in one family and
not the other would be no rule. The environment and the boundary move together or
not at all.

**Declining is one run's standing no**, remembered outside every document
([§8.6](#86-where-a-connector-grant-goes)) and retractable there.

**A machine MAY hold several connections of one connector**, including several of
one method. Two tokens for two accounts, or two sign-ins at different scopes, are
two connections. Each records the method that produced it, and each is
authenticated independently.

A connection's **authority** is what its `auth` exchange reported about what the
connection may do — OAuth scopes, in that mechanism's terms. It is opaque to this
format: the mechanism produces it, and `lns` stores and compares it without
interpreting it. A `kind: token` exchange reports none.

**A mechanism MUST report authority canonically.** The same permissions MUST
report identically, and anything that varies between exchanges — an expiry, a
token id, an issue time, the order of a set — MUST NOT be part of it.

**A grant names a method, and — where that method has `auth` — the connection that
supplies its values.** Two connections of one method may hold different authority, so
a run that granted one has not granted its sibling.

A method with no `auth` produces no connection. Its grant names the method alone, and
it is granted like any other: **a connector is never applied without a grant, with
or without authentication.**

**The card discloses the connection** — its label and its authority — not just
the method. This is the one thing on the card that no document contains, so it is
read from the connection rather than derived from the connector.

`lns` MUST ask again in two cases, for the same reason and with the same force:

- **A republished connector.** Any change is a different digest, so a grant bound
  to the old bytes does not carry over ([§7.1](#71-connectors)) — the whole digest,
  because any list of fields that can widen a grant is a list that can be missing
  one.
- **A connection whose authority changed.** A grant records the authority it
  consented to. Where a re-authentication returns anything different, every grant
  naming that connection MUST be invalidated. Difference is the test, not widening.

So the **connect** prompt escalates: it re-authenticates in place while the
authority comes back unchanged, and raises the consent card when it does not.

A grant MUST NOT be silently widened.

**A run holds at most one grant per connector, so it grants at most one
method.** Granting again replaces the prior grant: that method's payload is
retracted — egress closes, injections disarm, filesets are removed — and the new
one's applies. That is what keeps a connector one merge source
([§3.3.2](#332-merge-rules)), and it is how a run moves between methods and
between connections of one method.

**Granted is not connected.** The combination is reachable: `lns connector
disconnect` drops a connection and keeps the grants naming it, and reinstalling a
connector restores the document without restoring what it held
([§7.1](#71-connectors)). In that state:

- The granted method **is** the run's source in [§3.3.2](#332-merge-rules), so
  its `egress`, its `filesets`, and its `env` apply as the run consented.
  They needed no value.
- Its `credentials` ship **unarmed** — the placeholder is registered, no value
  substitutes for it. A request carrying the placeholder is held rather than
  relayed ([§4.1](#41-the-credential-definition)), and that hold raises the
  **connect** prompt.

A granted connection whose method has an `auth` but no `credentials` raises no prompt
and needs none — nothing holds, and nothing is missing. `lns connector connect` is
the only way to restore it.

#### 3.2.5 A fileset carries the placeholder, not the value

Some clients read a credential from a file rather than an environment variable. A
method serves them with a fileset whose content carries the **placeholder**:

```yaml
methods:
  - name: token
    auth:
      kind: token
    credentials:
      - envVar: SOME_TOKEN
        placeholder: some_LNSPLACEHOLDER0000000000
        injections:
          - kind: bearer_header
            domain: api.some-provider.example
    filesets:
      - guestPath: ~/.some-provider
        inline:
          credentials.json: '{"token":"some_LNSPLACEHOLDER0000000000"}'
```

The client reads the file and sends the placeholder. On a request to
`api.some-provider.example` the proxy **sets** the `Authorization` header to the
real value, replacing whatever the client sent — the header-family behaviour in
[§4.1](#41-the-credential-definition), reached by a file instead of a variable.
The real value stays on the host, which is what makes a fileset at a
secret-shaped name in the guest's home safe to allow.

Two rules govern it, and only the second is checkable:

- **A method's fileset MUST NOT carry a real secret.** No validator can verify
  this — nothing distinguishes a real token from arbitrary text. These documents
  are pushed to registries, so a value here is a credential one push away from
  being public. What compensates is exposure: every fileset a method writes is
  named on the card before the user grants it ([§1.5](#15-one-disclosure)), and
  every write is recorded.
- **A method MAY use a secret-shaped file name, and MUST declare a placeholder in
  every file that has one.** [§3.1.11](#3111-filesets) refuses names like
  `credentials.json`; a connector is the one kind whose filesets exist to write
  those exact files, and this rule earns the exception. A fileset with a
  secret-shaped name and no declared placeholder is refused. The placeholder MUST
  be one the same method declares, because a sibling's injection is not armed
  when this one applies.

A `connector` document carries the placeholder, never a value. Values stay
per-machine.

---

### 3.3 `kind: mixin`

A mixin layers a capability onto a sandbox, so it is authored and versioned once
instead of copied into every `lns.yaml`.

```yaml
apiVersion: lns.run/v1
kind: mixin
name: postgres-tools
spec:
  mixins:
    - ghcr.io/acme/psql-client@sha256:8d3f2b60c9e14a75f8b02d6c31e97a4b5f0d8c26a19e34b7d05f6c81a2e93b47

  tools:
    - postgresql@17
  egress:
    http:
      - match: deb.debian.org
        verdict: allow
        description: the mirror this mixin's pre-start script installs from
    tcp:
      - match: db.example.com:5432
        verdict: allow
  filesets:
    - inline:
        USING-POSTGRES.md: "Connect with $DATABASE_URL."
      guestPath: /home/agent/notes
  scripts:
    - when: pre-start
      user: root
      run: apt-get install -y --no-install-recommends postgresql-client
```

| Block | In a mixin |
|---|---|
| `env`, `egress`, `credentials`, `tools`, `volumes`, `filesets`, `ports`, `scripts` | Allowed, with the same rules as [§3.1](#31-kind-sandbox). |
| `mixins` | Allowed. A mixin MAY build on other mixins, exactly as a sandbox does. |
| `image`, `command`, `workdir`, `user`, `resources` | **FORBIDDEN.** These describe one launch, and the sandbox owns it. |

A mixin ships agent-facing markdown as a `fileset`, so instructions land in the
guest through the same mechanism as any other file and appear in the resolved
disclosure like one.

**`scripts` is why a mixin can carry a `user` per script while `user` itself is
forbidden.** The two name different things. `spec.user` is the workload's
identity — one launch, and the sandbox owns it, which is why a mixin may not
answer for it. A `scripts[].user` names who runs one script that travels with the
document that wrote it, and it settles nothing about the launch.

What keeps that from being a way around the forbidden block is not the narrowness
of the reach — a root script can write anywhere the mounts allow, and a
[bounded root](#313-user) is still root. It is that the consumer approves the
script. Every entry appears in the resolved sandbox before boot, attributed to the
document that contributed it and showing the user it asks for
([§1.5](#15-one-disclosure)) — so a mixin that wants root gets it by saying so to
the person running it, which is exactly what `spec.user` would have had to do.

One mixin is never authored: every run has [its own decisions](#8-the-runs-decisions)
that the run writes as the developer answers prompts.

#### 3.3.1 How a mixin enters a run

Two ways, and both end in the same resolved sandbox:

```bash
lns run claude                        # claude's own spec.mixins, if it declares any
lns run claude --mixin xyz --mixin zyx    # the user adds two more, in flag order
```

| Source | Written by | Reference form |
|---|---|---|
| [`spec.mixins`](#319-mixins) | The sandbox's author | A local path, or an OCI reference that MUST be digest-pinned. A published sandbox has to resolve to the same thing for everyone. |
| `--mixin <ref>` | The user, per run | A local path, or a reference that resolves like a sandbox reference — a tag is fine, and preflight pins and shows the digest before boot. |

The asymmetry is the same one that governs a sandbox reference itself. A digest
inside a published document is what makes it reproducible for a stranger; a
reference the user types is their own live choice, and they see what it resolved
to.

**A local path is a directory or the document itself.** A directory is read as
the `lns.yaml` inside it; a path naming the document is that document. These are
the two spellings `lns run` already takes for a local sandbox, and one reference
grammar means one rule — nothing about the merge distinguishes them, since a
path a document writes roots at the directory the document sits in either way.

A published sandbox MUST NOT name a local path — a consumer has no copy of the
author's working directory, so the reference resolves to nothing, or worse to
something else. This is a rule about the **published bytes**, and `lns push`
satisfies it by publishing the mixin the path names and pinning its digest
([§6.1](#61-a-local-mixin-publishes-with-the-document-that-names-it)); the
author's own document keeps the path. A `--mixin` may name one on any run,
published or not: the user typed it on the machine the run happens on, and the
preflight shows them what it resolved to.
A declared entry roots at the directory of the document that named it; a
`--mixin` roots where the user typed it, since no document names one. Either
way the folded absolute path of the **document** it resolved to is its identity —
so a directory and the `lns.yaml` inside it are one source, not two — and that
identity is what the graph keys on, and what
the disclosure shows.

Either way the mixin is pulled, merged, and shown in the resolved sandbox the run
presents for approval ([§1.5](#15-one-disclosure)). A `--mixin` is not a way to
skip that.

#### 3.3.2 Merge rules

Resolution flattens the sandbox and every mixin into **one ordered list of
sources**, then merges them in that order. **The last source to say something
about a thing wins.**

The list is built by walking, and a mixin's own `mixins` are appended right after
it:

1. The sandbox's own `spec` — the base every mixin layers onto.
2. Each entry of the sandbox's `spec.mixins`, in order; after each, that mixin's
   own `mixins`, expanded the same way.
3. Each `--mixin`, in flag order; after each, that mixin's own `mixins`, expanded
   the same way.
4. Each granted [connector](#32-kind-connector) — its **granted method** only, in
   grant order. User consent on this machine, so it beats anything a document
   shipped.
5. The run's own [decisions](#8-the-runs-decisions) — the developer's own
   answers, so neither what they pulled nor what they connected can overrule
   them.

So `lns run claude --mixin first --mixin second` resolves as:

```text
claude's spec  →  claude's spec.mixins  →  first  →  first's mixins  →  second  →  connectors  →  local
weakest ────────────────────────────────────────────────────────────────────────────────────────────▶ wins
```

**A connector is the one source no document names**, so it is the one source
whose position is not a consequence of where it was written down. It sits after
every document because the user granted it here and now; it sits before the
run's decisions because a `deny` the developer answered is still the last word.
A connector shadowed that way is reported in the approval window, rather than
silently taking no effect.

**A connector contributes one source, not one per method.** Methods are
alternatives ([§3.2.2](#322-methods)), so the source is the granted method, and
the others are not in the list at all. A grant is what puts it here; whether the
machine still holds that connection decides only whether its credentials are armed
([§3.2.4](#324-installing-connecting-and-applying)).

Two properties fall out of the shape:

- **A `--mixin` beats anything the document said**, because the user's flags are
  appended last. Same principle that lets `--cpus` outrank a declared `resources`.
- **A mixin's own mixins beat the mixin itself**, because they come after it. A
  mixin that pulls in another is asking for that other's version of a shared
  setting.

What "wins" means per block:

| Block | Merge |
|---|---|
| `env` | Union by key. The last source to set a key wins. |
| `egress` | Union of entries, later sources placed ahead of earlier ones, so the latest entry matching a destination is the one that decides ([§4.2](#42-the-egress-definition)). |
| `credentials` | Union by `envVar`, or by `placeholder` for an entry that declares no `envVar` ([§4.1](#41-the-credential-definition)). A later source redefining one replaces it whole — its `placeholder` and `injections` together, never half of each. |
| `tools` | Union by name. The last version declared wins. |
| `volumes`, `filesets` | Union by guest path — a volume `target` and a fileset `guestPath` share one namespace, compared on path segments ([§3.1.10](#3110-volumes)). The last source to claim a path owns it. A named volume's [`size`](#3110-volumes) is the largest any surviving entry declares, because a size is a floor and every mount of that volume must clear it. |
| `ports` | Union by `container`. The last mapping wins. |
| `scripts` | **Append**, in source order. Not last-wins: every source's scripts run, the sandbox's own first and the run's decisions last. |

**`scripts` is the one block nothing overrides**, and the reason is that it has
no key. Every other block names what it decides — an `envVar`, a tool, a guest
path — so a later source claiming that name is stating a newer answer to the same
question. Two scripts state no question. Dropping one because another looked like
it would not be an override; it would be a missing dependency, discovered as a
command not found somewhere inside the workload. So every script survives, and
order is the only thing resolution decides about them — the source order above,
which puts the sandbox's own preparation first and the run's decisions last. Later
still wins, in time rather than in precedence.

Because a source many documents name merges at the last place the order names it,
its scripts run there too — so a dependency two mixins share prepares **after**
both of them rather than before, and no earlier mention can move it. For every
other block that rule only decides precedence; for this one it decides sequence.
An author who needs one document's preparation to come first therefore cannot get
it by ordering alone: nothing later may name that document.

It also means the merged document can carry more scripts than
[§3.1.13](#3113-scripts) lets one author declare. That ceiling is authoring
feedback, not a bound on a run: appending is the rule, so a sum neither author
could see must not refuse the launch.

Uniqueness is a **per-document** rule: one document may not name the same
`envVar`, tool, guest path, or `container` port twice ([§3.1](#31-kind-sandbox)),
because nothing inside one file disambiguates them. Across sources, the same
collision is the override mechanism, not an error.

A `host` port is the one exception, because it is a machine resource rather than
a setting. Two sources publishing different `container` ports onto one `host`
port both want the same socket, and precedence cannot settle that — keeping the
later mapping would silently unpublish a port the sandbox declared. Resolution
**refuses**, naming both sources, so the author moves one.

Overriding is therefore normal and expected, which is exactly why the resolved
sandbox is presented before boot: the disclosure shows what each source
contributed and what it replaced, so an override nobody intended is visible while
it can still be refused ([§1.5](#15-one-disclosure)).

- **A mixin MAY declare mixins.** Resolution walks the graph to a depth of **5**
  and refuses beyond it, measured by the shortest path that reaches a source —
  so a mixin one document names deep does not refuse a graph another names
  shallow. A cycle — a mixin reachable from itself — refuses the run; a
  digest-pinned reference and a directory's folded absolute path both make one
  detectable by identity.
- **A source many documents name is one source.** It merges at the last place
  the order names it, and the disclosure names it once. An earlier appearance
  can decide nothing the last one does not: either a source after it sets a key,
  or the source itself sets it again.
- **Winning a merge is not a way past the developer.** A mixin's entries are
  enforced like any other, so what stops a mixin from quietly widening a run is
  disclosure: the resolved sandbox it produced is what the developer approves
  ([§1.3](#13-disclosure-before-boot)). A credential it contributes still has no
  value until one is bound ([§3.1.7](#317-credentials)).

Because resolution needs the network, a run whose mixins are not yet cached cannot
start offline — including a mixin reached only through another. A cached,
digest-pinned graph resolves offline: the digest is the whole identity, so there is
nothing left to look up.

---

## 4. Shared definitions

Two `spec` shapes appear in more than one kind. They are defined once here, and
each kind states only what it adds.

### 4.1 The credential definition

The one shape shared by all three kinds. A **credential** is the low-level
injection contract, and it is identical wherever it appears:

| Kind | Where |
|---|---|
| [`sandbox`](#317-credentials) | `spec.credentials[]` — the secrets its workload needs. |
| [`mixin`](#33-kind-mixin) | `spec.credentials[]` — the same, contributed to whatever sandbox resolves it. |
| [`connector`](#32-kind-connector) | `spec.methods[].credentials[]` — the same, supplied to whatever run grants a connection of that method. |

A connector uses this shape rather than defining one of its own, because it is a
higher-level way to *supply* a credential — the injection mechanism is here.

```yaml
envVar: SOME_TOKEN
placeholder: some_LNSPLACEHOLDER0000000000
injections:
  - kind: bearer_header
    domain: api.some-provider.example
```

| Field | Type | Rules |
|---|---|---|
| `envVar` | string | Conditional. The variable the workload sees. REQUIRED in a `sandbox` and a `mixin`. Optional inside a [method](#322-methods), where a credential may exist only to be injected on the wire — see below. |
| `placeholder` | string | REQUIRED. The literal value the workload reads. MUST self-identify as fake — it contains `placeholder` or `lns`, case-insensitively — and MUST be at least 16 characters, so a stream is unlikely to carry the marker by accident. |
| `field` | string | Conditional. Inside a [method](#322-methods), which of the method's `auth` outputs supplies this credential's value. Optional when the `auth.kind` produces exactly one; REQUIRED when it produces more. Refused outside a method, where there is no `auth` to draw from. |
| `injections` | list | optional. Where the real value replaces the placeholder, host by host. |
| `injections[].kind` | string | REQUIRED. See the two families below. |
| `injections[].domain` | string | REQUIRED. The destination this injection applies to, port included for a raw stream. A wildcard may name a family of hosts, but never the catch-all `*`: an injection states where a secret may travel, and a catch-all states nothing while putting the real value on every host the workload reaches. |
| `injections[].header` | string | Conditional. `api_key_header` only. |

**Why `envVar` is optional in a method and nowhere else.** A `sandbox` or a
`mixin` declares a credential because its workload needs one, and the workload
reads it from a variable — so the variable is the declaration. A method's
credential is a *supply*, and a supply can land somewhere else entirely: a
[fileset](#325-a-fileset-carries-the-placeholder-not-the-value) carrying the
placeholder serves a client that never reads the environment. Requiring `envVar`
there would force an author to invent a variable nothing reads, and an unread
variable holding a credential marker is worse than no variable.

A credential with no `envVar` still registers its `placeholder`, because
injection is domain-keyed and needs nothing else. The `placeholder` is then the
entry's identity: it is what [§3.3.2](#332-merge-rules) merges on, so no two
credentials **in one source** may share one. A `placeholder` is already required
to be distinctive, so this costs an author nothing and gives every entry a key.

Inside a connector the source is one [method](#322-methods), not the document.
Two methods are alternatives and only one ever enters the merge, so they may
share an `envVar` and share a `placeholder`, on the condition below. They usually
should: a token method and a sign-in method serve the same variable of the same
service.

**Two methods that declare one `envVar` MUST declare one `placeholder` for it.**
A run moves between methods, and a granted method's `env` reaches only the
workloads that start after the grant. So granting method B leaves every live
session of that run holding method A's marker. One placeholder per variable is
what keeps those sessions valid across the switch. A connector that breaks this
rule is refused wherever the document is parsed
([§5](#5-validation-summary)).

The rule runs one way only. Two methods MAY declare one `placeholder` under two
`envVar`s: each variable still holds one marker, so nothing is left undecided.

The definition names the shape of a secret without containing one. The real value
is bound per machine and lives outside every document
([§7.1](#71-connectors)); the placeholder is what the workload actually
holds, and the proxy substitutes the real value on the way out to a declared
`domain`.

That split is why injection is **domain-keyed**: a credential injected on the wire
works whether or not the workload ever reads `envVar`. Setting the variable exists
so a tool can detect that it is signed in.

Injection kinds come in two families, and the family decides which
[egress](#42-the-egress-definition) destinations an injection can serve:

| Family | Kinds | Works on |
|---|---|---|
| **Header** — the proxy sets or rewrites an HTTP header | `bearer_header`, `token_header`, `basic_x_access_token`, `api_key_header` | An `http` destination. The request has to be readable, so the traffic is TLS-terminated. |
| **Placeholder** — the proxy finds the placeholder in the outbound bytes and replaces it | `uri_placeholder` | Either table. A `tcp` destination is served this way: a database password, an SSH key passphrase, or a broker token reaches the wire without the workload ever holding the real value. |

The family also decides what a placeholder the injection did not reach means, and
the answer is that it is **held rather than relayed**. Injection runs first: on a
request to a domain some injection claims, the proxy applies every armed
injection and only then looks for a surviving marker. A credential whose
injection covered the request proceeds untouched by this rule.

What survives is a real mismatch, and there are two:

- The injection is **unarmed** — declared, with no value behind it. One state
  produces this: a [connector](#32-kind-connector) connection a run granted, on
  a machine that no longer holds it
  ([§3.2.4](#324-installing-connecting-and-applying)). A sandbox's or a mixin's
  own credential still cannot reach it: a bound declaration is armed before boot,
  and one left to a connector ([§3.1.7](#317-credentials)) declares nothing to the
  boundary at all until the grant registers it armed.
- The placeholder is somewhere the declared kind does not reach. A
  `bearer_header` injection sets one header; the same placeholder in a request
  body is untouched by it, so it survives. Reaching a body takes a
  `uri_placeholder` injection.

Either way the request is held instead of carrying the marker onto the wire,
because a marker on the wire tells a destination that a secret was meant to be
here and was not — and the request it belongs to would fail anyway, less
informatively.

Placeholder substitution is why a `placeholder` MUST be distinctive: it is the
marker the proxy looks for. A value that could occur naturally in a stream would
be replaced where it was never meant to be.

The consequence for a raw stream is worth stating plainly: the workload sends the
placeholder as its password, and the proxy swaps it in transit. The workload can
authenticate to a service whose credential it never had — the same guarantee HTTP
injection gives, applied to bytes rather than headers.

A `placeholder` MUST also look fake for a reason that survives publishing: these
documents are pushed to registries, so a placeholder that could pass for a real
token is a credential one edit away from being committed.

---

### 4.2 The egress definition

**Egress** is what a document says about reaching the outside: two ordered tables,
`http` and `tcp`. Every kind declares it as `spec.egress`, and the block is
**identical** in all three — same fields, same rules, same meaning. A reader who
knows one knows them all.

```yaml
egress:
  http:
    - match: api.some-provider.example
      verdict: allow
      scheme: https
      tlsTerminate: true
      rules:
        - method: GET
          path: /v1/models
  tcp:
    - match: db.some-provider.example:5432
      verdict: allow
```

`http[]` — a destination reached as HTTP requests:

| Field | Type | Rules |
|---|---|---|
| `match` | string | REQUIRED. The destination pattern. Grammar in [Policy](policy.md). |
| `verdict` | string | REQUIRED. `allow` or `deny`. |
| `transport` | string | optional. `direct`, the only value — the sandbox reaches the destination itself. |
| `scheme` | string | optional. `http` or `https`. |
| `tlsTerminate` | bool | optional. Default `false`. |
| `rules` | list | optional. Per-request narrowing: each entry MAY set `method` and `path`. |
| `binaries` | list<string> | optional. Absolute guest binary paths the entry is scoped to. |
| `description` | string | optional. Shown wherever the entry is explained. |

`tcp[]` — a destination reached as a raw stream:

| Field | Type | Rules |
|---|---|---|
| `match` | string | REQUIRED. Destination, port included. |
| `verdict` | string | REQUIRED. `allow` or `deny`. |
| `binaries` | list<string> | optional. As above. |
| `description` | string | optional. |

Three rules govern the pair:

- **`tcp` is matched first.** A destination it decides never reaches `http`. That
  is what makes a non-HTTP service — a database, an SSH host, a message broker —
  expressible at all.
- **Declaring `rules` implies `tlsTerminate`.** An HTTP method or path is only
  visible once the proxy terminates TLS, so a narrowing that could never be read
  would be an entry that silently does nothing.
- **`binaries`, when present, MUST be non-empty**, and each entry MUST name a
  binary the kernel can resolve. Paths are compared against `/proc/<pid>/exe`, so
  `/` and any path with a `..` segment can never match. Redundant separators and
  `.` segments are fine, because a `Path` comparison treats them as equal. An
  entry scoped to binaries decides for those callers and denies every other,
  rather than falling through to the next entry.

The identical block means the same thing from each kind, and the difference is
only who is speaking:

| Declared by | Reads as |
|---|---|
| [`sandbox`](#316-egress) / [`mixin`](#33-kind-mixin) | What this workload may reach. A destination no entry decides is asked about. |
| [`connector`](#32-kind-connector) | What this service needs in order to work, declared inside one [method](#322-methods). Granting a method grants every entry it declares, not only the one that raised the offer. |

**Combining egress from several sources.** A run's egress is rarely one table: a
sandbox, the mixins it resolves, and each granted connector method contribute one.
They are unioned, and one rule settles the overlap:

- **Within one document, the first matching entry decides.** An author writes an
  ordered table and reads it top to bottom.
- **Across sources, the later source decides.** Its entries are placed ahead of
  earlier ones in the merged table, so a mixin can widen or narrow what the
  sandbox said. Source order is the resolution order in
  [§3.3.2](#332-merge-rules).

A later source can therefore turn a `deny` into an `allow`. That is deliberate —
overriding is what layering is for — and it is why the merged table is disclosed
before boot rather than trusted silently: the developer approves the effective
egress, not each contributor's opinion of it.

---

## 5. Validation summary

Offline validation (`lns artifact validate`, and every load path including
`lns run` preflight) enforces, in addition to the per-field rules above:

- **Document**: `apiVersion` is `lns.run/v1`; `kind` is one of the three;
  `name` matches the name pattern; no unrecognized field at any level, with the
  one exception [§1.2](#12-strict-decoding) states — the body of a connector
  method's `auth` whose `kind` this reader does not know.
- **Sandbox**: `image` present and non-empty; `workdir` absolute with no `..`;
  `user` has at most one `:`, no empty segment, and no `=`, whitespace, control
  character, or quote.
- **env**: every key is a legal environment-variable name; within one source, no
  key is also a credential's `envVar` — one variable holds one value, and
  nothing downstream decides between a plain value and a placeholder.
- **resources**: an absolute `cpu` is a positive count and an absolute `memory`
  a parsable byte size; a share is a whole 1–100 with a `%` suffix; `disk` is a
  parsable byte size, at least `20Mi` and less than `16Ti`, and is not a share.
- **egress**: every entry sets `match` and a `verdict` of `allow` or `deny`; an
  `http` entry declaring `rules` terminates TLS; every `binaries` filter is
  non-empty and names kernel-resolvable paths.
- **credentials**: every `envVar` is a legal environment-variable name; every
  `placeholder` self-identifies as fake and is at least 16 characters; every
  `injections[].domain` is set and is not the catch-all `*`, and `header`
  appears only on `api_key_header`; within one source no two entries share an
  `envVar` and no two share a `placeholder` — one source being one document, or
  one [method](#322-methods) inside a connector; `envVar` is present outside a
  [method](#322-methods) and `field` is absent there; inside a method `field` is
  present when the method's `auth.kind` produces more than one value.
- **tools**: every entry parses as portable `name@version`; one entry per name.
- **volumes**: each `target` is a legal mount path outside `/.lens`; the
  `type`/`source`/`name` combination is one of the three legal shapes; volume
  names use the allowed charset; no duplicate `target`; `exclude` appears only on a
  bind and every entry is a relative path with no empty, `.`, or `..` segment;
  `optional` appears only on a bind; `size` appears only on a named volume and is
  a parsable byte size, at least `20Mi` and less than `16Ti`, and is not a share.
- **filesets**: exactly one of `path`, `inline`, or `hostPath`; inline paths and
  limits hold; no secret-shaped name; a `hostPath` is anchored, contained,
  literal, and names one file; `optional` appears only on a `hostPath`;
  `guestPath` is absolute or `~/`-anchored and never names another user's home,
  and is unique **as written** across volumes and filesets — two spellings of one
  resolved path are caught at launch ([§3.1.11](#3111-filesets)); no claim lands
  under a read-only volume, and none lands under a bind unless that same bind's
  `exclude` covers it — decided here for an absolute claim, and at launch for a
  `~/`-anchored one ([§3.1.11](#3111-filesets)).
- **ports**: `container` and `host` in range and each unique.
- **scripts**: every entry sets a `when` this grammar defines and a `run` that is
  non-empty and free of NUL; a `user`, where present, follows the `user` rule; at
  most 32 entries, each `run` at most 128 KiB, and 512 KiB across the block — the
  entry count and the block total bound one document rather than a resolved one,
  while the per-script size holds for both, since merging cannot grow a script.
- **Connector**: `serves` is present and non-empty; `methods` is present and
  non-empty; every method `name` matches the name pattern and is unique in the
  document; no method declares a block
  [§3.2.3](#323-what-a-method-may-carry) refuses, and no block a method may carry
  appears under `spec`; no `filesets[].hostPath`; every fileset `guestPath` is
  `~/`-anchored and a method's `inline` bytes total no more than 1 MiB
  ([§3.2.3](#323-what-a-method-may-carry)); an `auth`, where present, sets
  a `kind`; a method declaring `credentials` declares an `auth`; every placeholder self-identifies as fake and
  is at least 16 characters; no two methods declare one `envVar` with different
  placeholders ([§4.1](#41-the-credential-definition)); and every fileset file
  with a secret-shaped name declares a placeholder **its own method** also
  declares ([§3.2.5](#325-a-fileset-carries-the-placeholder-not-the-value)). That last
  check reads content, so it covers an `inline` value and a file in a `path`
  directory beside the document. A `path` a validator cannot read — a pulled
  artifact's packed layer — is checked at push, where the layer is built. An
  unknown `auth.kind` is **not** a validation failure
  ([§3.2.2](#322-methods)).
- **Mixin**: no `image`, `command`, `workdir`, `user`, or `resources`; every
  entry in a document's `mixins` is a local path or a digest-pinned OCI
  reference; and a document **loaded from a registry** names no local path,
  which is refused rather than resolved — a pulled document that carries one was
  not produced by `lns push`, and resolving it against the consumer's own
  directory is the "or worse, something else" of
  [§3.3.1](#331-how-a-mixin-enters-a-run). Whether a local entry is *publishable*
  is not an offline check on one document: it depends on the document that entry
  names ([§6.1](#61-a-local-mixin-publishes-with-the-document-that-names-it)).

Offline validation checks one document in isolation. Six checks cannot run
there, because they depend on state no document carries — they run at launch:

| Check | Depends on |
|---|---|
| Whether a declared credential has a value bound ([§3.1.7](#317-credentials)) | Per-machine credential values. |
| Which connector method this run has granted ([§3.2.4](#324-installing-connecting-and-applying)) | The machine's installed set, the connections it holds, and the run's own grants. |
| The host a `%` share resolves against ([§3.1.5](#315-resources)) | The host's total cores and RAM. |
| Whether a bind `source` ([§3.1.10](#3110-volumes)) or a `hostPath` is present, and whether a pulled `hostPath` is allowed ([§3.1.11](#3111-filesets)) | The running machine's files, and its recorded host-path decisions. |
| Whether two `guestPath` entries resolve to one path ([§3.1.11](#3111-filesets)) | The guest's home directory, which a `~/`-anchored path is resolved against. |
| Whether a `~/`-anchored fileset claim lands under a volume `target` ([§3.1.11](#3111-filesets)) | The guest's home directory, for the same reason. |
| The resolved source list in [§3.3.2](#332-merge-rules) | The mixin graph — its depth, its cycles, and which source wins each setting — is only known once each mixin, and each mixin it declares, is pulled. |

The last one is why a merge collision refuses the **run** rather than the
document: `lns artifact validate` cannot see it.

---

## 6. Publish-time transforms

`lns push` publishes the document with three resolutions applied, so a consumer
runs exactly what the author tested:

| Surface | Transform |
|---|---|
| `filesets[].path` | The directory is packed into a layer of this artifact. The entry keeps its `path` and `guestPath`; the content is now part of the artifact's digest. |
| `tools[]` | A fuzzy version (`node@22`, `python@latest`) is resolved against the tool's public version index and rewritten exact. |
| `mixins[]` local entry | The document it names publishes first, as its own artifact, and the entry is rewritten to that artifact's digest ([§6.1](#61-a-local-mixin-publishes-with-the-document-that-names-it)). A digest-pinned entry publishes untouched. |
| `README.md` | A `README.md` beside the document is packed into a `text/markdown` layer of this artifact ([§7.2](#72-the-readme-layer)). No file, no layer; the document itself never carries it. |

Each transform pins something that means one thing on the author's machine and
another on the consumer's: a directory that exists only beside the author's file,
a version that moves next week, and a document only the author has a copy of. Two
surfaces stay unresolved, on purpose:

- **A `%` share stays a share.** It resolves against the consumer's host, which
  is the entire point of writing one ([§3.1.5](#315-resources)).
- **A `hostPath` stays a `hostPath`.** There is nothing to pack: it names a file
  on whichever machine runs the document, and the consumer decides whether it is
  read ([§3.1.11](#3111-filesets)).

`workdir`, every volume, every fileset, and every other field publish unchanged.

`lns push --dry-run` performs everything short of the upload and prints the
digests that would publish, for every artifact the push would create. It stays
offline, so it does **not** resolve tool versions — in any of them — and it says
when declared tools mean the real digest may differ from the preview.

### 6.1 A local mixin publishes with the document that names it

A published document MUST NOT carry a local path
([§3.3.1](#331-how-a-mixin-enters-a-run)). Publish makes that true by publishing,
not by refusing: the mixin the entry names becomes its own artifact, and the entry
becomes that artifact's digest. The author writes one document and runs one
command; the consumer receives a digest-pinned graph.

**The repository is not invented.** Two things the author already wrote decide it:
the `<REF>` they typed, up to but not including its last path segment, and the
mixin's own [`name`](#2-common-top-level-fields). Any tag or digest on the
`<REF>` is dropped first, and a bare `<REF>` is qualified against the machine's
registry default before the rule runs — so the mixin always lands in the same
registry as the document that names it. Nothing derives a name from a directory,
because a repository is a publishing decision and a directory name is not one.

| `<REF>` | The mixin `postgres-tools` publishes at |
|---|---|
| `ghcr.io/acme/dev:1.4` | `ghcr.io/acme/postgres-tools` |
| `ghcr.io/acme/team/dev:1.4` | `ghcr.io/acme/team/postgres-tools` |
| `hub.lns.run/dev` | `hub.lns.run/postgres-tools` |

| Rule | |
|---|---|
| Order | Children publish before the documents that pin them, so every digest exists before it is referenced. A mixin MAY name mixins of its own; the depth limit is [§3.3.2](#332-merge-rules)'s, measured the same way — by shortest path, so publish never refuses a graph a run would resolve. |
| Identity | The folded absolute path of the resolved **document**, as in [§3.3.1](#331-how-a-mixin-enters-a-run). Two entries that resolve to one document publish once, and **every** entry naming it is pinned to that one digest. |
| Tag | `sha256-<64 hex>`, where the hex is the child's manifest digest. It is **not a release**; only the tag the author types on `<REF>` is a release. |
| Source | The author's document is **not** rewritten. Only the published bytes carry the digest, exactly as a resolved `tools[]` entry does. |
| Cycle | Refused, naming the trail. A document reachable from itself has no digest to pin, because its digest would depend on itself. |
| A run's decisions | Refused. [§8.1](#81-what-it-is) makes a run's `decisions.yaml` never published, and an entry naming it cannot be honoured by publishing it. A saved copy ([§8.4](#84-how-a-decision-leaves-the-run)) is an ordinary document and publishes like one. |

**Why the child carries a tag at all.** The parent resolves it by digest and
never reads the tag. The tag exists because a registry that prunes untagged
manifests could otherwise reclaim a mixin a published sandbox still pins.
Deriving the tag from the digest keeps it immutable in practice: the same bytes
always claim the same tag, and different bytes never collide, so it is a durable
name rather than a moving one. Because it is not a release, it carries no
`org.opencontainers.image.version` annotation
([§7](#7-distribution)) — and it could not carry one even if it were: that
annotation sits inside the bytes the tag's own name derives from, so recording it
there is impossible.

The author keeps a working directory they can edit, and the consumer gets a graph
they can resolve. Startup resolution is unaffected: the consumer's run pulls each
pinned mixin and merges it exactly as [§1.5](#15-one-disclosure) describes.

---

## 7. Distribution

Every kind is an OCI artifact, published individually. The document is the config
blob, and the media type names the kind:

| Kind | `artifactType` | Config `mediaType` |
|---|---|---|
| `sandbox` | `application/vnd.lns.sandbox.v1+json` | `application/vnd.lns.sandbox.config.v1+json` |
| `connector` | `application/vnd.lns.connector.v1+json` | `application/vnd.lns.connector.config.v1+json` |
| `mixin` | `application/vnd.lns.mixin.v1+json` | `application/vnd.lns.mixin.config.v1+json` |

Every kind carries one layer per `filesets[].path` entry it declares
([§3.1.11](#3111-filesets)), plus at most one README layer
([§7.2](#72-the-readme-layer)). Nothing else is addressable on its own, so one
reference names one complete, digest-pinned thing. A consumer correlates fileset
layers by their media type, in manifest order, and MUST leave out any layer whose
media type it does not consume rather than guess at it.

A release is a reference, not a field: `lns push ghcr.io/acme/reviewer:1.4.0`
makes that tag the release, and push records it as the artifact's
`org.opencontainers.image.version` annotation so any OCI tool can read it. One
answer, in the place the registry already looks.

Consumers SHOULD pin by digest. `lns run` preflight pins the resolved artifact
digest and asks before running any declared tool's installer; approval applies
to the digest that preflight showed, so a tag that moves before the pull is
refused.

### 7.1 Connectors

A connector is pulled and **installed** on a machine. The installed set is what
can be offered ([§1.4](#14-credentials-and-what-a-connector-adds)) — nothing
ships inside the `lns` binary. A machine with no connectors installed still runs
every sandbox; each declared credential is simply asked for directly, as
[§3.1.7](#317-credentials) describes.

Installing is neither connecting nor granting. Installing alone applies nothing:
what applies a method is a run's grant, and what arms its credentials is a
connect ([§3.2](#32-kind-connector)). So installing bytes a run already holds a
matching grant for — a reinstall of what it consented to — applies the granted
method again to that run: at once where the run is running, by the same blocks
[§3.2.4](#324-installing-connecting-and-applying) lets a grant apply mid-run, and
at its next start otherwise. It asks for a connect when a request needs a value.

Five things live per machine, none of them in any document:

| What | Keyed by | Scope |
|---|---|---|
| The installed set — each connector's document, stored verbatim at the digest it came from | name | The machine |
| Each **connection** — its label, the method `name` that produced it, the authority its `auth` reported, and the values it returned | name, then connection | The machine |
| Which method a run granted, the connection behind it where the method authenticates, and the authority it consented to | run, then name, digest | The run |
| Which connectors a run declined | run, then name | The run |
| A grant **reserved** for a name no run holds yet | run name, then connector, digest | That name, until a run takes it |

The second row admits many connections per connector, including many of one method
([§3.2.4](#324-installing-connecting-and-applying)). The third names the method a
run consented to, and the connection behind it where one exists — a method with
no `auth` has none. The third and fourth are keyed by the run itself and not by
its name, so a name that `lns sandbox rm` frees carries nothing to the next run
that takes it.

The fifth is the one row keyed by a run's *name* rather than by the run itself,
because it is written before the run it is for exists.
[§3.2.4](#324-installing-connecting-and-applying) states what takes it — creating
a run with an explicit name, and nothing else — and the two rules that stop it
becoming a standing consent: a reservation is consumed once, and an
automatically generated name never takes one.

A connector that is installed and neither granted nor declined by a run holds no
row of its own, and still changes that run in two ways. Its `serves` destinations
ask ([§3.2.1](#321-serves)), and a declaration it can still answer holds its
`placeholder` ([§3.1.7](#317-credentials)).

**The two keys differ deliberately.** A connection's key starts with the connector
name and survives an update; a grant is keyed by the digest too and does not, and
a reservation is a grant in this respect. So a connector republished with a
changed `egress` is offered again — the user is asked to consent to the new bytes
— while every connection stays signed in.
Re-consenting is cheap; signing in again is not, and making an update log the
user out would teach them to avoid updates.

Storing the document **verbatim, at its digest**, is what makes that possible: it
is what a grant binds to.

Three rules follow from a connector arriving over the network:

- **Any update offers again.** A grant is bound to the digest, so a connector
  republished with any change has no grant here and the user is asked. It MUST
  NOT silently widen one — a widened domain would ship a token somewhere the
  developer never approved, and enumerating which fields could do that is how a
  list comes to be missing one ([§3.2.4](#324-installing-connecting-and-applying)).
- **An overlapping `serves` refuses the install, not the run.** Installing a
  connector whose `serves` overlaps an installed connector's fails at install
  time, where the user can act on it, rather than leaving an ambiguous offer to
  surface at some later launch ([§3.2.1](#321-serves)). Overlap is what
  conflicts, not equality: a connector serving `*.some-provider.example` and one
  serving `api.some-provider.example` cannot both be installed.
- **A variable two connectors both claim refuses the install too**, for the same
  reason and at the same moment. This covers an `envVar` — one variable holds one
  placeholder, and a second claim would make injection ambiguous — and a plain
  `env` key. The check is across connectors only: two **methods** of one
  connector may claim the same variable — with one `placeholder` between them
  ([§4.1](#41-the-credential-definition)) — because they are alternatives and
  only one is ever applied ([§3.2.2](#322-methods)). Within **one** source the rule
  holds again and is not an install-time check at all: a source that sets a
  variable through `env` and also fills it from a credential claims one variable
  twice, and is refused wherever the document is parsed
  ([§5](#5-validation-summary)) — only its author can say which of the two
  the workload was meant to read.

**Uninstalling stops the offer; it does not retract a grant.** A run that
already granted a method keeps that decision, and reinstalling the same digest
resumes that grant with no fresh consent prompt — the grant was bound to those
bytes and those bytes came back. This is deliberate: uninstalling is housekeeping
on the machine, and withdrawing consent is a decision about a run. A run
retracts its own grant, or its decline, through the store that holds it
([§8.6](#86-where-a-connector-grant-goes)).

**Uninstalling does drop every connection**, because they are machine state and the
machine is what is being cleaned. So a reinstall resumes the grant and **not** the
connection: the run's consent stands, the machine is asked to connect again,
and the run behaves as [§3.2.4](#324-installing-connecting-and-applying) describes
for a granted connection the machine no longer holds.

See [Distributing a sandbox](running-workloads.md#distributing-a-sandbox) for
the `lns push` / `lns pull` / `lns tag` workflow.

### 7.2 The README layer

A `README.md` beside the document publishes as a layer of the same artifact:

```json
{
  "mediaType": "text/markdown",
  "digest": "sha256:…",
  "size": 4096,
  "annotations": { "org.opencontainers.image.title": "README.md" }
}
```

The layer is how a registry UI renders documentation for the artifact, and being
a layer is the point: every digest carries exactly the README that shipped with
it, so a pinned reference never shows documentation from a different version, and
updating the README is a push like any other — there is no out-of-band edit that
can drift from the artifact it describes.

The file is found by its exact name, `README.md`, in the directory of the
document being published — each artifact of one push looks beside its own
document, so a local mixin ships its own README or none. The file is capped at
1 MiB; a larger one refuses the push. No file means no layer, never an empty one.

The README never enters the guest. It is not a fileset, it is not mounted, and a
consumer that materializes filesets leaves it out like any other layer whose
media type it does not consume ([§7](#7-distribution)). Relative links inside it
resolve against nothing; a README SHOULD use absolute URLs.

---

## 8. The run's decisions

Every run has one mixin nobody wrote. The run fills it in as the developer works.

A destination no `egress` entry decides is asked about
([§1.3](#13-disclosure-before-boot)). The answer is a decision, and an unrecorded
decision is the same question again tomorrow — so the run writes it down, as an
`egress` entry, in a `mixin` document, in the run's own directory.

```yaml
apiVersion: lns.run/v1
kind: mixin
name: decisions
spec:
  egress:
    http:
      # written when the developer approved this destination
      - match: docs.some-vendor.example
        verdict: allow
      - match: proxy.some-vendor.example
        verdict: deny
```

### 8.1 What it is

- **A `mixin`.** The same grammar as [§3.3](#33-kind-mixin), so a decision is
  recorded in the shape the rest of the document already defines and stays
  readable by the person who made it.
- **One run's, and always present.** A run has one whether or not anything was
  asked, and it resolves without being named. It is created with the run and
  removed with it, so a run that is gone has no decisions left behind.
- **Not the directory's.** Two runs started in one directory each answer for
  themselves, and neither reads the other's file. The working directory decides
  which paths a reference resolves against and nothing else
  ([§8.5](#85-what-the-working-directory-decides)).
- **Written by the run.** An approval appends to it. It holds only what is
  **new** — the decisions the developer made about things nothing had decided.
  Anything a `sandbox` or a `mixin` already declares stays in that document, where
  its author can see it.
- **Local, and never published.** The one exception to
  [§1.1](#11-one-distribution-mechanism): every other artifact is addressed by
  digest, and this one is a working file on disk. It holds one machine's answers
  about one run, so a `mixins` entry that names it is refused rather than
  published ([§6.1](#61-a-local-mixin-publishes-with-the-document-that-names-it)).
  Reading it is not publishing it: a `--mixin` that names one is an ordinary
  local reference to an ordinary mixin. Saving it is how a decision travels to
  another machine ([§8.4](#84-how-a-decision-leaves-the-run)).
- **Last in the merge.** It is the developer's own, so it sits after every other
  source in [§3.3.2](#332-merge-rules) — including a `--mixin`. Nothing they pulled
  can overrule what they decided, and that includes what this file itself pulls: a
  mixin it names merges **before** it, not after, so the general rule that a mixin's
  own mixins beat it stops at this one.

The developer does not approve this file. [§1.5](#15-one-disclosure) has a run
disclose what it resolved before booting, which protects them from documents they
did not write; this one is written from their own answers. Its contents still
appear in what the run reports, so "why did this run reach that host" keeps an
answer.

### 8.2 Why a mixin rather than a format of its own

Because it makes the file worth reading. A decision recorded in mixin grammar is a
decision a developer can open, correct, and diff — and one they can **keep**, by
writing the run out as a document of its own
([§8.4](#84-how-a-decision-leaves-the-run)), so a project's agreed destinations are
reviewable in a pull request instead of rediscovered by each developer alone. A
bespoke policy format would need its own parser, its own documentation, and its
own answer to every question [§3](#3-artifact-kinds) already answers.

It also stops the file being egress-only. If a decision ever needs to record
something other than a destination, the blocks are already defined.

### 8.3 The name

The file is `decisions.yaml`, in the run's own directory. It holds decisions, in
mixin grammar, and may hold more than egress — so it says what the file **is**
rather than where it once sat.

Its `name` is the file's own stem, because nobody is present to choose one.

### 8.4 How a decision leaves the run

A decision is worth keeping past the run that made it. The run's directory is
temporary: it goes when the run does.

So a run can be written out as a document the developer names and puts where they
want ([`cli-spec.md` §3.2.3](cli-spec.md#323-the-runs-decisions-and-saving-them)).

- Written as a `mixin`, the document holds what the run decided and nothing else.
- Written as a `sandbox`, it holds what the **documents** resolved to: image,
  command, `env`, `workdir`, `user`, mounts, ports, resources, tools, filesets,
  credentials, and the `egress` every document source decided, the run's own
  decisions included. A mixin the run resolved is folded in, so the saved
  document names none and resolves nothing; what it came from is in the run's
  audit record, not in the file.
- **A grant stays out of it.** What a connector granted is source 4
  ([§3.3.2](#332-merge-rules)). It is one person's consent on one machine
  ([§8.6](#86-where-a-connector-grant-goes)), so a saved `sandbox` MUST NOT carry
  it. A teammate running the saved document is asked on their own machine.
- **Every path is written absolute.** A relative `source` means the directory of
  the document that declared it ([§3.3.1](#331-how-a-mixin-enters-a-run)), and a
  saved document lands somewhere else. So a save MUST write the folded absolute
  path the run resolved. The document then means the same thing from any
  directory.
- Either way the document's `name` is the stem of the file it is written to,
  because someone is present to choose one — unlike the run's own
  `decisions.yaml` ([§8.3](#83-the-name)). A stem that is not a legal
  [`name`](#2-common-top-level-fields) MUST refuse the save rather than write a
  document [§5](#5-validation-summary) would reject.

From that point it is an ordinary document: validated, rendered, published, and
named by a `mixins` entry like any other.

**That is how a decision travels.** A rule applies because a document names it,
never because a file sits in a directory. A team commits the saved mixin and the
`sandbox` that needs it lists it in `spec.mixins`
([§3.3.2](#332-merge-rules)) — the same resolution as every other mixin, with no
path that only works from one directory.

### 8.5 What the working directory decides

The working directory decides nothing about a run's contents. It decides nothing
about consent.

It roots the relative paths the user types on the command line: a `-f`, a
`--mixin`, a `-v` source, an `--env-file`. A relative `source` inside a document
roots at the directory of the document that declared it, not at the working
directory ([§3.3.1](#331-how-a-mixin-enters-a-run)).

One thing follows: a rule applies because a document names it.

### 8.6 Where a connector grant goes

A connector grant does not live here, and this is the one place where "a
connector is a mixin" stops short of the obvious. Writing the granted
connector's reference into this file's `mixins` would make every later run
resolve it by the ordinary rules, with no special path anywhere — which is
exactly why it is tempting, and exactly why it is wrong.

**This file can be saved out** ([§8.4](#84-how-a-decision-leaves-the-run)), and a
saved document is committable. A grant is consent to let a real value reach a real
destination ([§7.1](#71-connectors)), given by one person on one machine, and
consent does not travel in a git clone. A teammate is asked on their own machine
rather than inheriting an answer. Worse, the answer would not even work there:
where a grant names a connection, that connection is this machine's and no other
machine holds it.

So a grant lives in the store the machine already keeps
([§7.1](#71-connectors)), keyed by the run — and, before that run exists, by the
name the user will give it. The run reads it and merges the granted method at the
position [§3.3.2](#332-merge-rules) gives it — the same merge, from a different
source.

**The working directory decides nothing about consent**
([§3.2.4](#324-installing-connecting-and-applying)). Two runs in one directory
each answer for themselves. A run's decisions follow the same rule
([§8.1](#81-what-it-is)), so one rule governs both.

A decline lives beside it, and both are retractable: a run can forget what it
decided about one connector, so the next run asks again. Neither is a document,
so neither is committed, and a mis-clicked deny is never permanent.

---

## 9. See also

- [Running workloads](running-workloads.md) — the authoring guide for this format.
- [Policy and approvals](policy.md) — the `match` pattern grammar and the
  run's own `decisions.yaml`.
- [CLI reference](cli-reference.md) — `lns artifact init`, `validate`, `inspect`,
  `push`.
