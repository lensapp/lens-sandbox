# Sandbox definition specification — `lns.run/v1`

Normative reference for the `lns.run/v1` document format: the `lns.yaml` a
developer writes, the connector that supplies a credential, and the mixin that
layers a capability onto a sandbox.

> **Status: the target, not today.** This document is the agreed destination for
> the format. It describes that destination as settled — nothing below hedges
> about implementation state — and parts of it do not ship yet.
>
> For what `lns` accepts today, read the guides:
> **[Running workloads](running-workloads.md)** for authoring and publishing,
> **[Policy and approvals](policy.md)** for the per-directory policy file,
> **[Connectors](connectors.md)** and **[Credentials](credentials.md)** for
> reaching a service.
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
| **`connector`** | How a credential for a service is obtained and injected: its credentials, the egress that reaches it, and a real sign-in. |
| **`mixin`** | A capability layered onto a sandbox: tools, filesets, egress, credentials. |

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
Installing it cannot arm it — that takes a sign-in
([§3.2.2](#322-seeding-arming-and-domain-ownership)).

### 1.4 Credentials, and what a connector adds

A **credential** is the mechanism ([§4.1](#41-the-credential-definition)). It is the
single description the proxy works from: this placeholder, in this variable,
replaced by the real value on a request to this domain. Nothing else injects a
secret, so a credential is the truth about where a secret can go, and a `sandbox`
or `mixin` declares as many as it needs.

A credential is complete on its own. Given one, the run knows what to ask the
developer for — a token, pasted once — and knows exactly where the value may
travel afterwards.

A **connector** ([§3.2](#32-kind-connector)) is a bundle built on that same
mechanism, and it is worth having for three reasons a bare credential cannot
cover:

| A connector adds | Why it matters |
|---|---|
| One or more credentials, published together | The env var, placeholder, and injection kinds for a service are stated once by whoever knows them, instead of by every author who talks to it. |
| The egress that service needs | Reaching the destination and authenticating to it are one decision, so connecting arrives with its own egress rather than a second round of approvals. |
| A real sign-in — OAuth device flow or PKCE | Better than copy-paste: no token pasted by hand, scoped grants, and a value that can be refreshed. |

So a connector is a convenience and a security upgrade over pasting, never a
different mechanism. A sandbox declares the credential it needs; which method
supplies the value — a pasted token, or a connector's sign-in — is the user's
decision on their own machine.

### 1.5 One disclosure

A consumer approves one thing before an untrusted sandbox runs, and composition
MUST preserve that.

A mixin resolves into the sandbox at **startup**, so the document a consumer
pulls is not yet the sandbox that boots. What the consumer approves is therefore
the **resolved** sandbox: before boot, the run presents the merged result in full
— every rule, mount, file, tool, script, and credential, each attributed to the
mixin it came from. Whatever the reference looked like, the approval is against a resolved
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
| [`connector`](#32-kind-connector) | The user, on their machine | Installed once, then offered when a run needs a credential it covers. |
| [`mixin`](#33-kind-mixin) | Anyone | Referenced by a sandbox, merged at startup. |

The middle column is the distinction to keep in view. A `sandbox` and a `mixin`
travel **with the workload** — an author writes them, and a consumer approves what
they say. A `connector` belongs to **the machine**: the user installs it, and no
sandbox can name one ([§1.4](#14-credentials-and-what-a-connector-adds)). All
three are artifacts because they are published and pinned alike, not because the
same person writes them.

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
credential-shaped placeholder — see [§3.1.7](#317-credentials) and
[Credentials](credentials.md).

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
workload does not work without it, so the run resolves every declaration before
boot:

| At boot | Outcome |
|---|---|
| A value is bound on this machine | The workload starts with `placeholder` in `envVar`, and the proxy substitutes the real value on requests to each declared `domain`. |
| No value is bound | The run asks the developer for one, naming the `envVar` and the domains it will reach. Answering binds the value and the run continues; declining refuses the run. |

Asking before boot is the value of writing the credential down: the alternative is
a workload that starts, runs, and fails somewhere inside itself with an unset
variable.

**An installed connector makes that question better, not different.** When a
connector covers a declared `domain` ([§3.2.2](#322-seeding-arming-and-domain-ownership)),
the prompt offers its sign-in — an OAuth device flow or PKCE, with scoped grants
and a refreshable value — instead of a pasted token. Accepting also brings that
connector's egress. With no connector installed, the same declaration still works;
the developer pastes a token, and the injection is identical.

A declaration never carries the value. Per-machine credential values live outside
every document — see [Credentials](credentials.md).

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
what was behind it does not.

| Rule | Detail |
|---|---|
| Relative | Each entry is relative to `source`. Every segment MUST be non-empty and MUST NOT be `.` or `..`, which refuses a leading `/`, a trailing `/`, and `a//b` in one rule. |
| Exact | An entry names one path, not a pattern. There are no globs. |
| Absent is fine | An entry naming a path that is not there does nothing. |
| Author's rule, not a decision | An exclusion is part of the document, so it is never prompted for and never recorded as a per-machine choice. |

An exclusion travels with the sandbox: a published bind masks what its author
meant, on whatever machine runs it.

A developer who wants to mask something the author did not name puts a bind with
their own `exclude` in the [local mixin](#8-the-local-mixin). It is last in the
merge, so a pulled document cannot undo it.

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
| `hostPath` | string | Conditional. One file on the machine that runs the document. See the rules below. |
| `guestPath` | string | REQUIRED. Where the files land in the guest. Same rules as a volume `target` ([§3.1.10](#3110-volumes)). A `hostPath` entry carries one file, so its `guestPath` MUST NOT end in `/`. |
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
names starting with `.env` or `credentials.`, names ending in `.pem`, `.key`,
`.ppk`, or `.keystore`, and the exact names `.npmrc`, `.netrc`,
`.git-credentials`, `.pgpass`, `.pypirc`, `.yarnrc.yml`, `auth.json`,
`credentials`, the SSH key names (`id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519`),
and the credential directories `.ssh`, `.aws`, `.gnupg`, `.kube`, `.azure`,
`.oci`, `.docker`.

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
not in the [local mixin](#8-the-local-mixin): the local mixin is a rule the
directory keeps, and this is a risk accepted by one developer on one computer.

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

A connector bundles the three things a service needs in one installable document:
its [credentials](#41-the-credential-definition), the
[egress](#42-the-egress-definition) that reaches it, and a real sign-in that
obtains the value ([§1.4](#14-credentials-and-what-a-connector-adds)).
It adds no injection mechanism of its own — the credential is the mechanism.

```yaml
apiVersion: lns.run/v1
kind: connector
name: some-provider              # the connector id
spec:
  displayName: Some Provider
  egress:
    http:
      - match: api.some-provider.example
        verdict: allow
  methods:
    - authKind: oauth            # credential | oauth
      oauth:
        flow: device
        deviceAuthorizationEndpoint: https://some-provider.example/device/code
        tokenEndpoint: https://some-provider.example/oauth/token
        envVar: SOME_OAUTH_TOKEN
        placeholder: some_LNSPLACEHOLDER0000000000
        injections:
          - kind: bearer_header
            domain: api.some-provider.example
    - authKind: credential       # paste a token instead
      help: https://some-provider.example/settings/tokens
      credential:
        envVar: SOME_TOKEN
        placeholder: some_LNSPLACEHOLDER0000000000
        injections:
          - kind: bearer_header
            domain: api.some-provider.example
```

| Field | Type | Rules |
|---|---|---|
| `displayName` | string | optional. The label on cards and prompts. Falls back to `name`. |
| `egress` | map | optional. An [egress definition](#42-the-egress-definition) — the destinations this service needs, HTTP and raw-TCP alike. Owned by the connector rather than by a method: every method reaches the same service. |
| `methods` | list | REQUIRED, non-empty. The ways this connector can obtain a value. |

#### 3.2.1 `methods[]`

One entry is one way to obtain a value. A connector that supports a real sign-in
**and** a pasted token declares two methods — an `oauth` one and a `credential`
one — rather than treating the paste as a property of the sign-in.

| Field | Type | Rules |
|---|---|---|
| `authKind` | string | REQUIRED. `credential` or `oauth`. The matching block MUST be present. |
| `credential` | map | Conditional on `authKind: credential`. |
| `oauth` | map | Conditional on `authKind: oauth`. |
| `displayName` | string | optional. How this method is named when the user picks between them. |
| `help` | string | optional. A URL where the user obtains a value by hand. |
| `command` | string | optional. A host command that mints one, shown for the user to run. |

`help` and `command` describe how a person gets a value for **this** method, so
they are most useful on a `credential` method — an `oauth` flow already walks the
user through it. They live on the connector and not on the shared
[credential definition](#41-the-credential-definition) on purpose: a connector is
installed deliberately by the user, whereas a `sandbox` is pulled, and a pulled
document that could put an arbitrary command in front of the developer is a
phishing surface.

**Two methods MAY share an `envVar`.** Two ways of obtaining a token for one
service usually produce interchangeable tokens, and the workload reads the same
variable either way. What distinguishes methods is how they obtain the value, not
where it lands — so the machine records *which method the user chose*, and never
infers it from a variable name.

`credential` is a [credential definition](#41-the-credential-definition),
unchanged. `oauth` is the same definition plus the mechanics of obtaining the
value:

| Field | Type | Rules |
|---|---|---|
| `flow` | string | optional. `device` (RFC 8628, default) or `pkce`. |
| `clientId` | string | optional. Supports `${ENV_VAR}` interpolation, so a real client id is never committed. A build that ships none cannot run this flow, and the user picks a `credential` method instead. |
| `clientSecret` | string | optional. Confidential device clients only. Same interpolation. |
| `scopes` | list<string> | optional. |
| `deviceAuthorizationEndpoint` | string | REQUIRED when `flow: device`. |
| `authorizationEndpoint` | string | REQUIRED when `flow: pkce`. |
| `tokenEndpoint` | string | REQUIRED. |
| `userinfoEndpoint` | string | optional. |
| `accountField` | string | optional. The userinfo field naming the signed-in account. |

#### 3.2.2 Seeding, arming, and domain ownership

- **A method seeds its placeholder when it is the method this machine holds.**
  A connector with one method always holds it, so it always seeds. A connector
  with several seeds nothing until the user picks one — including the case where
  the choice is between a real sign-in and pasting a token.
- **Seeding is not connection.** Routes and arming stay gated by the connect
  card, so an unconnected connector's seeded placeholder is still held at the
  gate. Env seeding exists so a tool can detect "I am signed in"; injection
  itself is domain-keyed, not placeholder-keyed.
- **One connector per domain.** Two connectors claiming the same destination is
  refused at install time, because the match in [§3.1.7](#317-credentials) would
  be ambiguous. See [§7.1](#71-connectors).
- **The offer fires on a domain match, once per workload — even when a network
  rule already allows the domain.** A decline is a per-workload standing no.
- **`envVar`, every `injections[].domain`, and every `egress` `match` are
  approval-relevant.** A pulled update that changes one MUST invalidate the
  approval and re-prompt. It MUST NOT silently widen it.

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

One mixin is never authored: every directory has a [local one](#8-the-local-mixin)
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
4. The directory's [local mixin](#8-the-local-mixin) — the developer's own
   decisions, so nothing they pulled can overrule them.

So `lns run claude --mixin first --mixin second` resolves as:

```text
claude's spec  →  claude's spec.mixins  →  first  →  first's mixins  →  second  →  local
weakest ─────────────────────────────────────────────────────────────────────────────▶ wins
```

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
| `credentials` | Union by `envVar`. A later source redefining one replaces it whole — its `placeholder` and `injections` together, never half of each. |
| `tools` | Union by name. The last version declared wins. |
| `volumes`, `filesets` | Union by guest path — a volume `target` and a fileset `guestPath` share one namespace. The last source to claim a path owns it. A named volume's [`size`](#3110-volumes) is the largest any surviving entry declares, because a size is a floor and every mount of that volume must clear it. |
| `ports` | Union by `container`. The last mapping wins. |
| `scripts` | **Append**, in source order. Not last-wins: every source's scripts run, the sandbox's own first and the local mixin's last. |

**`scripts` is the one block nothing overrides**, and the reason is that it has
no key. Every other block names what it decides — an `envVar`, a tool, a guest
path — so a later source claiming that name is stating a newer answer to the same
question. Two scripts state no question. Dropping one because another looked like
it would not be an override; it would be a missing dependency, discovered as a
command not found somewhere inside the workload. So every script survives, and
order is the only thing resolution decides about them — the source order above,
which puts the sandbox's own preparation first and the local mixin's last. Later
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
| [`connector`](#321-methods) | Inside each sign-in method, as the credential that method obtains a value for. |

A connector uses this shape rather than defining one of its own, because it is a
higher-level way to *obtain* a credential — the injection mechanism is here.

```yaml
envVar: SOME_TOKEN
placeholder: some_LNSPLACEHOLDER0000000000
injections:
  - kind: bearer_header
    domain: api.some-provider.example
```

| Field | Type | Rules |
|---|---|---|
| `envVar` | string | REQUIRED. The variable the workload sees. |
| `placeholder` | string | REQUIRED. The literal value the workload reads. MUST self-identify as fake — it contains `placeholder` or `lns`, case-insensitively — and MUST be at least 16 characters, so a stream is unlikely to carry the marker by accident. |
| `injections` | list | optional. Where the real value replaces the placeholder, host by host. |
| `injections[].kind` | string | REQUIRED. See the two families below. |
| `injections[].domain` | string | REQUIRED. The destination this injection applies to, port included for a raw stream. A wildcard may name a family of hosts, but never the catch-all `*`: an injection states where a secret may travel, and a catch-all states nothing while putting the real value on every host the workload reaches. |
| `injections[].header` | string | Conditional. `api_key_header` only. |

The definition names the shape of a secret without containing one. The real value
is bound per machine and lives outside every document
([Credentials](credentials.md)); the placeholder is what the workload actually
holds, and the proxy substitutes the real value on the way out to a declared
`domain`.

That split is why injection is **domain-keyed**: a credential injected on the wire
works whether or not the workload ever reads `envVar`. Seeding the variable exists
so a tool can detect that it is signed in.

Injection kinds come in two families, and the family decides which
[egress](#42-the-egress-definition) destinations an injection can serve:

| Family | Kinds | Works on |
|---|---|---|
| **Header** — the proxy sets or rewrites an HTTP header | `bearer_header`, `token_header`, `basic_x_access_token`, `api_key_header` | An `http` destination. The request has to be readable, so the connection is TLS-terminated. |
| **Placeholder** — the proxy finds the placeholder in the outbound bytes and replaces it | `uri_placeholder` | Either table. A `tcp` destination is served this way: a database password, an SSH key passphrase, or a broker token reaches the wire without the workload ever holding the real value. |

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
| [`connector`](#32-kind-connector) | What this service needs in order to work. Accepting the connector accepts its entries. |

**Combining egress from several sources.** A run's egress is rarely one table: a
sandbox, the mixins it resolves, and every accepted connector each contribute one.
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

Offline validation (`lns sandbox validate`, and every load path including
`lns run` preflight) enforces, in addition to the per-field rules above:

- **Document**: `apiVersion` is `lns.run/v1`; `kind` is one of the three;
  `name` matches the name pattern; no unrecognized field at any level.
- **Sandbox**: `image` present and non-empty; `workdir` absolute with no `..`;
  `user` has at most one `:`, no empty segment, and no `=`, whitespace, control
  character, or quote.
- **env**: every key is a legal environment-variable name.
- **resources**: an absolute `cpu` is a positive count and an absolute `memory`
  a parsable byte size; a share is a whole 1–100 with a `%` suffix; `disk` is a
  parsable byte size, at least `20Mi` and less than `16Ti`, and is not a share.
- **egress**: every entry sets `match` and a `verdict` of `allow` or `deny`; an
  `http` entry declaring `rules` terminates TLS; every `binaries` filter is
  non-empty and names kernel-resolvable paths.
- **credentials**: every `envVar` is a legal environment-variable name; every
  `placeholder` self-identifies as fake and is at least 16 characters; every
  `injections[].domain` is set and is not the catch-all `*`, and `header`
  appears only on `api_key_header`; no two entries share an `envVar`.
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
  `guestPath` unique across volumes and filesets.
- **ports**: `container` and `host` in range and each unique.
- **scripts**: every entry sets a `when` this grammar defines and a `run` that is
  non-empty and free of NUL; a `user`, where present, follows the `user` rule; at
  most 32 entries, each `run` at most 128 KiB, and 512 KiB across the block — the
  entry count and the block total bound one document rather than a resolved one,
  while the per-script size holds for both, since merging cannot grow a script.
- **Connector**: at least one method; each method carries the block its
  `authKind` names and, for `oauth`, the endpoint its `flow` needs; every
  placeholder self-identifies as fake and is at least 16 characters.
- **Mixin**: no `image`, `command`, `workdir`, `user`, or `resources`; every
  entry in a document's `mixins` is a local path or a digest-pinned OCI
  reference; and a document **loaded from a registry** names no local path,
  which is refused rather than resolved — a pulled document that carries one was
  not produced by `lns push`, and resolving it against the consumer's own
  directory is the "or worse, something else" of
  [§3.3.1](#331-how-a-mixin-enters-a-run). Whether a local entry is *publishable*
  is not an offline check on one document: it depends on the document that entry
  names ([§6.1](#61-a-local-mixin-publishes-with-the-document-that-names-it)).

Offline validation checks one document in isolation. Five checks cannot run
there, because they depend on state no document carries — they run at launch:

| Check | Depends on |
|---|---|
| Whether a declared credential has a value bound ([§3.1.7](#317-credentials)) | Per-machine credential values. |
| Whether a sign-in is held ([§3.2.2](#322-seeding-arming-and-domain-ownership)) | Per-machine credential values. |
| The host a `%` share resolves against ([§3.1.5](#315-resources)) | The host's total cores and RAM. |
| Whether a bind `source` ([§3.1.10](#3110-volumes)) or a `hostPath` is present, and whether a pulled `hostPath` is allowed ([§3.1.11](#3111-filesets)) | The running machine's files, and its recorded host-path decisions. |
| The resolved source list in [§3.3.2](#332-merge-rules) | The mixin graph — its depth, its cycles, and which source wins each setting — is only known once each mixin, and each mixin it declares, is pulled. |

The last one is why a merge collision refuses the **run** rather than the
document: `lns sandbox validate` cannot see it.

---

## 6. Publish-time transforms

`lns push` publishes the document with three resolutions applied, so a consumer
runs exactly what the author tested:

| Surface | Transform |
|---|---|
| `filesets[].path` | The directory is packed into a layer of this artifact. The entry keeps its `path` and `guestPath`; the content is now part of the artifact's digest. |
| `tools[]` | A fuzzy version (`node@22`, `python@latest`) is resolved against the tool's public version index and rewritten exact. |
| `mixins[]` local entry | The document it names publishes first, as its own artifact, and the entry is rewritten to that artifact's digest ([§6.1](#61-a-local-mixin-publishes-with-the-document-that-names-it)). A digest-pinned entry publishes untouched. |

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
| The local mixin | Refused. [§8.1](#81-what-it-is) makes `lns-local-mixin.yaml` never published, and an entry naming it cannot be honoured by publishing it. |

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
| `sandbox` | `application/vnd.lens.sandbox.v1+json` | `application/vnd.lens.sandbox.config.v1+json` |
| `connector` | `application/vnd.lens.connector.v1+json` | `application/vnd.lens.connector.config.v1+json` |
| `mixin` | `application/vnd.lens.mixin.v1+json` | `application/vnd.lens.mixin.config.v1+json` |

A `sandbox` or `mixin` artifact carries one layer per `filesets[].path` entry it
declares ([§3.1.11](#3111-filesets)); a `connector` is config-only. Nothing else is
addressable on its own, so one reference names one complete, digest-pinned thing.

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
offers a better sign-in than a pasted token
([§1.4](#14-credentials-and-what-a-connector-adds)) — nothing ships inside the
`lns` binary. A machine with no connectors installed still runs every sandbox;
each declared credential is simply asked for directly, as
[§3.1.7](#317-credentials) describes.

Installing is not connecting. A newly installed connector holds no sign-in, so it
seeds nothing and arms nothing until the developer completes one
([§3.2.2](#322-seeding-arming-and-domain-ownership)).

Two rules follow from a connector arriving over the network:

- **An update that changes an approval-relevant field re-prompts.** `envVar`,
  every `injections[].domain`, and every `egress` `match` are what an approval is
  bound to. A pulled update that changes one MUST invalidate the approval, and it
  MUST NOT silently widen it — a widened domain would ship a token somewhere the
  developer never approved.
- **A domain claim conflict refuses the install, not the run.** Installing a
  connector that claims a destination an installed one already claims fails at
  install time, where the user can act on it, rather than leaving an ambiguous
  match to surface at some later launch ([§3.2.2](#322-seeding-arming-and-domain-ownership)).

See [Distributing a sandbox](running-workloads.md#distributing-a-sandbox) for
the `lns push` / `lns pull` / `lns tag` workflow.

---

## 8. The local mixin

Every directory has one mixin nobody wrote. The run fills it in as the developer
works.

A destination no `egress` entry decides is asked about
([§1.3](#13-disclosure-before-boot)). The answer is a decision, and an unrecorded
decision is the same question again tomorrow — so the run writes it down, as an
`egress` entry, in a `mixin` document, in the directory the work is happening in.

```yaml
apiVersion: lns.run/v1
kind: mixin
name: lns-local-mixin
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
- **Always present.** A directory has one whether or not anyone created it, and
  every run there resolves it without being named.
- **Written by the run.** An approval appends to it. It holds only what is
  **new** — the decisions the developer made about things nothing had decided.
  Anything a `sandbox` or a `mixin` already declares stays in that document, where
  its author can see it.
- **Local, and never published.** The one exception to
  [§1.1](#11-one-distribution-mechanism): every other artifact is addressed by
  digest, and this one is a working file on disk. It holds one machine's answers,
  so a `mixins` entry that names it is refused rather than published
  ([§6.1](#61-a-local-mixin-publishes-with-the-document-that-names-it)).
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
decision a developer can open, correct, and diff — and one they can **commit**, so
a project's agreed destinations are reviewable in a pull request instead of
rediscovered by each developer alone. A bespoke policy format would need its own
parser, its own documentation, and its own answer to every question
[§3](#3-artifact-kinds) already answers.

It also stops the file being egress-only. If a decision ever needs to record
something other than a destination, the blocks are already defined.

### 8.3 The name

The file is `lns-local-mixin.yaml`. The old name described a file that held
policy; this one holds decisions, in mixin grammar, and may hold more than
egress — so it says what the file **is** rather than what it once carried, and it
sorts beside the `lns.yaml` it layers on.

Its `name` is the file's own stem, because nobody is present to choose one.

### 8.4 Where a connector grant goes

A connector grant does not live here. A connector is installed **per machine**
([§7.1](#71-connectors)), while consenting to use one is **per project** — and
neither is something a mixin can say, because no artifact names a connector
([§1.4](#14-credentials-and-what-a-connector-adds)).

So it lives in the per-workload grant store the machine already keeps, keyed by
project alone: `lns connector connect` names a directory and no workload. Keeping it out of this file is what lets the file be committed — a
clone asks for its own consent rather than inheriting one nobody in that clone
gave.

---

## 9. See also

- [Running workloads](running-workloads.md) — the authoring guide for this format.
- [Policy and approvals](policy.md) — the `match` pattern grammar and the
  per-directory `lns-local-mixin.yaml`.
- [Credentials](credentials.md) — placeholders, and the per-machine values a
  credential resolves against.
- [Connectors](connectors.md) — connecting a workload to an external service.
- [CLI reference](cli-reference.md) — `lns sandbox init`, `validate`, `inspect`,
  `push`.
