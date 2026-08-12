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

A **kit** is one YAML document with an `apiVersion`, a `kind`, a `name`, and a
`spec`, published as one OCI artifact. Three kinds exist:

| Kind | Purpose |
|---|---|
| **`sandbox`** | A complete sandbox: the base image, the egress it needs, the credentials it needs, and the files, tools, and mounts it brings. |
| **`connector`** | How a credential for a service is obtained and injected: its credentials, the egress that reaches it, and a real sign-in. |
| **`mixin`** | A capability layered onto a sandbox: tools, filesets, egress, credentials. |

[Chapter 3](#3-kits) specifies each in full.

### 1.1 One distribution mechanism

Every kit is published and consumed the same way: **one document, one OCI
artifact, addressed by reference and pinned by digest** ([§7](#7-distribution)).

The uniformity is what makes kits composable. A sandbox reference, a connector
reference, and a mixin reference are the same kind of thing to a reader, to
`lns pull`, and to an approval — each independently versioned, each auditable at
its digest, each fixable by publishing a new one.

`lns run` boots a sandbox. `lns push` publishes any kit. `lns pull` and
`lns run <reference>` consume the published form. The same document is both the
authoring surface and the wire format.

### 1.2 Strict decoding

Decoding rejects unrecognized fields at every level of every document. Misspell a
field — `mountpath` for `mountPath`, or `egres` for `egress` — and the load fails
on that line instead of ignoring the key and running with a default. `lns sandbox
validate` runs the whole schema and cross-field check offline — no service, no
network.

### 1.3 Disclosure before boot

A run does what its kit says. `spec.egress` is **enforced as written** — an entry
that allows a destination allows it, and a mixin that narrows one narrows it. The
safeguard is not that a document is advisory; it is that the developer sees the
resolved document, in full, before anything boots
([§1.5](#15-one-disclosure)).

The same holds for `spec.ports`: a declared port is published, whether the
sandbox came from a registry or from the directory in front of you.

Two things stay outside a document's reach, because no declaration can supply
them:

- **A destination nothing decides.** An entry decides its match; a destination no
  entry covers is asked about at first use, so silence is never an allow.
- **A secret.** `spec.credentials` names the variable and the domains a value may
  travel to, and nothing more. The value is bound per machine.

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
— every rule, mount, tool, and credential, each attributed to the mixin it
came from. Whatever the reference looked like, the approval is against a resolved
digest, so what boots is what was approved
([§3.3.1](#331-how-a-mixin-enters-a-run)).

Startup resolution is what makes a mixin worth having: a fix to a mixin reaches
every sandbox that references it on the next run, without republishing any of
them. See [§3.3](#33-kind-mixin).

---

## 2. Common top-level fields

Every kit sets these, whatever its kind.

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

**A kit carries no version of its own.** The reference it was pushed to is the
release — `ghcr.io/acme/reviewer:1.4.0`, and the digest under it — so a `version`
field would be a second answer to the same question, free to disagree with the tag
it shipped under. Nothing in this document reads one: resolution, approval, and
merging all key on the digest. See [§7](#7-distribution).

The same name pattern applies to every identifier in every document: connector
ids, tool names, and mixin names.

**One API group.** Every kit is `lns.run/v1`, and the three kinds are all of them
— so one reader, and one grammar, covers everything a machine pulls or a developer
writes.

[Chapter 3](#3-kits) specifies the three kinds.
[Chapter 4](#4-shared-definitions) defines the two `spec` shapes they share.

---

## 3. Kits

A **kit** is one published document of one kind. The three kinds are the three
things a user assembles a run from, and the term names what they have in common:
each is authored, published, versioned, and approved the same way
([§1.1](#11-one-distribution-mechanism)).

> The name is provisional. It is the term Docker uses for the same idea, kept
> here until we pick our own.

| Kind | Whose document it is | How it enters a run |
|---|---|---|
| [`sandbox`](#31-kind-sandbox) | The workload's author | `lns run <reference>`, or a local `./lns.yaml`. Exactly one. |
| [`connector`](#32-kind-connector) | The user, on their machine | Installed once, then offered when a run needs a credential it covers. |
| [`mixin`](#33-kind-mixin) | Anyone | Referenced by a sandbox, merged at startup. |

The middle column is the distinction to keep in view. A `sandbox` and a `mixin`
travel **with the workload** — an author writes them, and a consumer approves what
they say. A `connector` belongs to **the machine**: the user installs it, and no
sandbox can name one ([§1.4](#14-credentials-and-what-a-connector-adds)). All
three are kits because they are published and pinned alike, not because the same
person writes them.

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
      mountPath: /etc/reviewer
    - path: ./prompts
      mountPath: /opt/reviewer/prompts
      owner: root

  ports:
    - container: 8080
      host: 18080
```

Read against the rules below, that document is valid: the image is digest-pinned,
`user: node` resolves in the guest's own `passwd`, both `egress` tables carry a
`verdict` on every entry, each credential's `injections` name a destination the
egress reaches, both mount shapes appear with distinct targets, and no `envVar`,
tool, mount target, or port repeats.

| Field | Required | Summary |
|---|---|---|
| [`image`](#311-image) | **REQUIRED** | The base OCI image the sandbox runs. |
| [`command`](#312-command-and-workdir) | optional | Replaces the image's default command; keeps its `ENTRYPOINT`. |
| [`workdir`](#312-command-and-workdir) | optional | Absolute guest working directory. |
| [`user`](#313-user) | optional | The user the workload runs as. |
| [`env`](#314-env) | optional | Non-secret environment variables. |
| [`resources`](#315-resources) | optional | vCPUs and memory, absolute or as a share of the host. |
| [`egress`](#316-egress) | optional | Where the workload may reach. |
| [`credentials`](#317-credentials) | optional | The secrets the workload needs and the domains they may reach. |
| [`tools`](#318-tools) | optional | Portable `name@version` toolchain declarations. |
| [`mixins`](#319-mixins) | optional | Mixin references merged into this sandbox at startup. |
| [`volumes`](#3110-volumes) | optional | Named volumes and host binds. |
| [`filesets`](#3111-filesets) | optional | Files shipped inside the artifact. |
| [`ports`](#3112-ports) | optional | Container ports the sandbox serves. |

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

A size is absolute or a **share of the host**.

```yaml
resources:
  cpu: 80%        # or 4, or "500m"
  memory: 50%     # or 512Mi
```

| Field | Type | Rules |
|---|---|---|
| `cpu` | int \| string | optional. An integer MUST be ≥ 1. A string is a positive count with an optional suffix (`500m`) or a share (`80%`). |
| `memory` | int \| string | optional. An integer MUST be ≥ 1. A string is a byte size (`512Mi`, `2Gi`) or a share (`50%`). |

A share lets one published artifact size itself sensibly on whatever machine
runs it. Its rules:

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

#### 3.1.9 `mixins`

```yaml
mixins:
  - ./mixins/postgres-tools/
  - ghcr.io/acme/postgres-tools@sha256:<64 hex>
```

| Field | Type | Rules |
|---|---|---|
| `mixins` | list<string> | optional. A local directory or an OCI reference. A remote reference MUST be digest-pinned. |

The list **publishes as written** — `lns push` does not merge it. Each mixin is
pulled and merged at startup, and the run presents the resolved sandbox for
approval before it boots ([§1.5](#15-one-disclosure)). A user can add more for one
run with `--mixin` ([§3.3.1](#331-how-a-mixin-enters-a-run)); the merge rules are
in [§3.3.2](#332-merge-rules).

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
```

| Field | Type | Rules |
|---|---|---|
| `type` | string | optional. `bind` or `volume`. See the shape table below for the omitted case. |
| `source` | string | Conditional. The host path for a bind; the volume name for `type: volume`. |
| `name` | string | Conditional. The volume name. MUST NOT appear on a bind. |
| `target` | string | REQUIRED. Absolute guest mount path. |
| `readOnly` | bool | optional. Default `false`. |
| `exclude` | list<string> | optional. Bind only. Subpaths of `source` the guest MUST NOT see. |

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

`target` rules, shared with [`filesets`](#3111-filesets):

- MUST be absolute, MUST NOT contain a `..` segment, MUST NOT contain control
  characters.
- MUST NOT overlap the `/.lens` runtime namespace, which belongs to the sandbox
  itself. `/` is likewise refused.
- MUST be unique across **all** `volumes` and `filesets` in the document.

#### 3.1.11 `filesets`

Files the document ships and the guest mounts.

```yaml
filesets:
  - inline:
      config.json: '{"mode":"review"}'
    mountPath: /home/agent/.agent
  - path: ./skills
    mountPath: /home/agent/.agent/skills
    owner: root
```

| Field | Type | Rules |
|---|---|---|
| `path` | string | Conditional. A directory beside this document, packed at publish. Non-empty. |
| `inline` | map<string,string> | Conditional. Relative path → file content. MUST NOT be empty. |
| `mountPath` | string | REQUIRED. Same rules as a volume `target` ([§3.1.10](#3110-volumes)). |
| `owner` | string | optional. `workload` (default) or `root`. |

**Exactly one** of `path` or `inline` MUST be set. Neither or both is an error.

A fileset is **not a separate artifact**. `inline` content lives in this
document, and a `path` directory is packed into a layer of the same artifact this
document configures, so the files and the declaration that mounts them share one
digest and are approved together. To share one directory across several
sandboxes, publish a [mixin](#33-kind-mixin) that carries it.

`owner: workload` lets the workload rewrite its own seeded state.
`owner: root` pins inputs the workload MUST NOT touch.

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
a path segment naming one fails the document. The check covers
names starting with `.env` or `credentials.`, names ending in `.pem`, `.key`,
`.ppk`, or `.keystore`, and the exact names `.npmrc`, `.netrc`,
`.git-credentials`, `.pgpass`, `.pypirc`, `.yarnrc.yml`, `auth.json`,
`credentials`, the SSH key names (`id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519`),
and the credential directories `.ssh`, `.aws`, `.gnupg`, `.kube`, `.azure`,
`.oci`, `.docker`.

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
    tcp:
      - match: db.example.com:5432
        verdict: allow
  filesets:
    - inline:
        USING-POSTGRES.md: "Connect with $DATABASE_URL."
      mountPath: /home/agent/notes
```

| Block | In a mixin |
|---|---|
| `env`, `egress`, `credentials`, `tools`, `volumes`, `filesets`, `ports` | Allowed, with the same rules as [§3.1](#31-kind-sandbox). |
| `mixins` | Allowed. A mixin MAY build on other mixins, exactly as a sandbox does. |
| `image`, `command`, `workdir`, `user`, `resources` | **FORBIDDEN.** These describe one launch, and the sandbox owns it. |

A mixin ships agent-facing markdown as a `fileset`, so instructions land in the
guest through the same mechanism as any other file and appear in the resolved
disclosure like one.

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
| [`spec.mixins`](#319-mixins) | The sandbox's author | MUST be digest-pinned. A published sandbox has to resolve to the same thing for everyone. |
| `--mixin <ref>` | The user, per run | Resolves like a sandbox reference — a tag is fine, and preflight pins and shows the digest before boot. |

The asymmetry is the same one that governs a sandbox reference itself. A digest
inside a published document is what makes it reproducible for a stranger; a
reference the user types is their own live choice, and they see what it resolved
to.

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
| `volumes`, `filesets` | Union by mount target. The last source to claim a target owns it. |
| `ports` | Union by `container`. The last mapping wins. |

Uniqueness is a **per-document** rule: one document may not name the same
`envVar`, tool, mount target, or `container` port twice ([§3.1](#31-kind-sandbox)),
because nothing inside one file disambiguates them. Across sources, the same
collision is the override mechanism, not an error.

Overriding is therefore normal and expected, which is exactly why the resolved
sandbox is presented before boot: the disclosure shows what each source
contributed and what it replaced, so an override nobody intended is visible while
it can still be refused ([§1.5](#15-one-disclosure)).

- **A mixin MAY declare mixins.** Resolution walks the graph to a depth of **5**
  and refuses beyond it. A cycle — a mixin reachable from itself — refuses the run;
  digest-pinned references make one detectable by identity.
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
  a parsable byte size; a share is a whole 1–100 with a `%` suffix.
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
  bind and every entry is a relative path with no empty, `.`, or `..` segment.
- **filesets**: exactly one of `path` or `inline`; inline paths and limits hold; no
  secret-shaped name; `mountPath` unique across volumes and filesets.
- **ports**: `container` and `host` in range and each unique.
- **Connector**: at least one method; each method carries the block its
  `authKind` names and, for `oauth`, the endpoint its `flow` needs; every
  placeholder self-identifies as fake and is at least 16 characters.
- **Mixin**: no `image`, `command`, `workdir`, `user`, or `resources`; every
  reference in
  a document's `mixins` is digest-pinned.

Offline validation checks one document in isolation. Four checks cannot run
there, because they depend on state no document carries — they run at launch:

| Check | Depends on |
|---|---|
| Whether a declared credential has a value bound ([§3.1.7](#317-credentials)) | Per-machine credential values. |
| Whether a sign-in is held ([§3.2.2](#322-seeding-arming-and-domain-ownership)) | Per-machine credential values. |
| The host a `%` share resolves against ([§3.1.5](#315-resources)) | The host's total cores and RAM. |
| The resolved source list in [§3.3.2](#332-merge-rules) | The mixin graph — its depth, its cycles, and which source wins each setting — is only known once each mixin, and each mixin it declares, is pulled. |

The last one is why a merge collision refuses the **run** rather than the
document: `lns sandbox validate` cannot see it.

---

## 6. Publish-time transforms

`lns push` publishes the document with two resolutions applied, so a consumer
runs exactly what the author tested:

| Surface | Transform |
|---|---|
| `filesets[].path` | The directory is packed into a layer of this artifact. The entry keeps its `path` and `mountPath`; the content is now part of the artifact's digest. |
| `tools[]` | A fuzzy version (`node@22`, `python@latest`) is resolved against the tool's public version index and rewritten exact. |

Each transform pins something that means one thing on the author's machine and
another on the consumer's: a directory that exists only beside the author's file,
and a version that moves next week. Two surfaces stay unresolved, on purpose:

- **`mixins[]` publishes as written.** Resolution is a startup concern
  ([§1.5](#15-one-disclosure)). The reference is already digest-pinned, so there
  is nothing for publish to pin.
- **A `%` share stays a share.** It resolves against the consumer's host, which
  is the entire point of writing one ([§3.1.5](#315-resources)).

`workdir`, every mount declaration, and every other field publish unchanged.

`lns push --dry-run` performs everything short of the upload and prints the
digests that would publish. It stays offline, so it does **not** resolve tool
versions, and it says when declared tools mean the real digest may differ from
the preview.

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
name: reviewer-local
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
  [§1.1](#11-one-distribution-mechanism): every other kit is an artifact addressed
  by digest, and this one is a working file on disk.
- **Last in the merge.** It is the developer's own, so it sits after every other
  source in [§3.3.2](#332-merge-rules) — including a `--mixin`. Nothing they pulled
  can overrule what they decided.

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
[§3](#3-kits) already answers.

It also stops the file being egress-only. If a decision ever needs to record
something other than a destination, the blocks are already defined.

### 8.3 Open: the name

`lns-policy.yaml` described a file that held policy. It holds decisions now, in
mixin grammar, and may hold more than egress. The name has to change; what it
changes to is not settled.

### 8.4 Open: where a connector grant goes

The same file records which connectors are connected today, and that record has no
home here. A connector is installed **per machine**
([§7.1](#71-connectors)), while consenting to use one is **per project** — and
neither is something a mixin can say, because no kit names a connector
([§1.4](#14-credentials-and-what-a-connector-adds)). So the grant needs a store of
its own, per project and outside the kit grammar. Where that lives is not settled.

---

## 9. See also

- [Running workloads](running-workloads.md) — the authoring guide for this format.
- [Policy and approvals](policy.md) — the `match` pattern grammar and the
  per-directory `lns-policy.yaml`.
- [Credentials](credentials.md) — placeholders, and the per-machine values a
  credential resolves against.
- [Connectors](connectors.md) — connecting a workload to an external service.
- [CLI reference](cli-reference.md) — `lns sandbox init`, `validate`, `inspect`,
  `push`.
