# Running workloads

Lens Sandbox has one user-facing noun: the **sandbox**. A sandbox is defined by a
`./lns.yaml` file that pins a base OCI image plus its command, environment, policy,
and connectors. One directory is one sandbox. A sandbox is either **cached**
(pulled or built, sitting in the local cache) or **running**.

You drive it on two tiers:

- The **top level** carries the docker-familiar verbs — `lns run`, `lns ps`,
  `lns stop`, `lns pull`, `lns push`, and so on. Each (except `run`) is an exact
  shortcut into the `lns sandbox` namespace.
- **`lns sandbox <verb>`** is the complete surface. The lns-native verbs that have
  no docker analogue — `validate`, `ls`, `prune` — live only there.

The background service must be running first (`lns service start`).

## Defining a sandbox

Scaffold a `./lns.yaml` in the current directory with `lns init` (a shortcut for
`lns sandbox init`):

```bash
lns init
```

```yaml
apiVersion: lns.run/v1
kind: sandbox
metadata:
  name: sandbox
spec:
  image: docker.io/library/alpine:3.20
  command: sh
  workdir: /workspace
  env: {}
  resources:
    cpu: 1
    memory: 512Mi
  policy:
    egress:
      http: []
  connectors: []
  credentials: []
  volumes:
    - type: bind
      source: .
      target: /workspace
  filesets: []
  ports: []
  tools: []
```

The scaffold carries every `spec` field with its default value, so the file
validates and runs as written: the current directory is bound at `/workspace`
and every network request asks for approval.

The `spec` fields:

| Field          | Meaning                                                                     |
| -------------- | --------------------------------------------------------------------------- |
| `image`        | The base OCI image the sandbox runs (**required**). Pin it by digest before publishing. |
| `command`      | Command to run in the workload, replacing the image's default command.       |
| `workdir`      | Absolute guest working directory. It is created when missing.                 |
| `user`         | The run-as user the sandbox needs, `USER[:GROUP]` like `-u`, so a definition that needs root is runnable as published. A per-run `-u` still wins, and the image's own `USER` is the fallback when this is unset. |
| `env`          | Non-secret environment variables seeded into the workload.                   |
| `policy`       | The network policy — the `egress.http` and `egress.tcp` rule tables (see [Policy](policy.md)). |
| `connectors` | Ids of the [connectors](connectors.md) the sandbox would like to use. Declaring seeds the connector's placeholder env var but is not a grant: a declared id is offered on first use (accept its connect card to arm it), never armed automatically — so an untrusted published sandbox can't open a route or spend a bound credential behind your back. An id the machine's catalog doesn't know refuses the launch. |
| `credentials`  | The secrets the workload needs, one entry each: the variable it reads (`envVar`), the fake value it holds (`placeholder`, which must contain `placeholder` or `lns` and be at least 16 characters), and the destinations the real value may travel to (`injections[]`, each a `kind` and a `domain`, which may name a host family but never the catch-all `*`). An `api_key_header` injection also names the `header` it sets. A declaration names no connector — this machine decides how the value is obtained. A connector whose own claim covers a declared domain supplies it: an `oauth`-kind one blocks the launch on its sign-in, a credential-kind one binds through the ordinary first-use value decision. With no catalog entry claiming the domain, the first request asks for a pasted value. Two entries may not share an `envVar`. See [Credentials](credentials.md#value-decisions). |
| `resources`    | vCPUs and memory the sandbox boots with (`cpu`, `memory` with a unit suffix, or `N%` of the host); per-run `--cpus` / `--mem` flags win. |
| `volumes`      | Named volumes and host binds mounted into the guest; a bind may `exclude` subpaths it must not expose. See [Declarative mounts](#declarative-mounts). |
| `filesets`     | Files shipped inside the artifact (`inline`, a `path` packed and digest-pinned at push, or a pre-published digest-pinned `ref`), snapshot-mounted at `mountPath` and owned by the workload user unless pinned with `owner: root`; see [Filesets](#filesets--files-shipped-inside-the-artifact). |
| `ports`        | Container ports the sandbox serves (`container`, optional `host`), validated offline. Running your own `./lns.yaml` publishes them automatically (compose-style, on loopback); a pulled sandbox's declared ports are disclosure only until you opt in with `-P` — see [Publishing ports](#publishing-ports). |
| `tools`        | Developer tools the workload needs, as portable `name@version` entries (`node@22`, `python@3.12`, `node@latest`). A version is required, and engine syntax (`aqua:`, `npm:`) is refused — the spec stays portable. Validated offline; the service provisions declared tools once per machine before boot, outside workload policy, and `lns push` pins fuzzy versions exact — see [Tools](#tools--declared-toolchains). |

Check the definition offline — no network, no service — with `validate` and a
target-less `lns inspect`:

```bash
lns sandbox validate     # schema, cross-field, and secret checks -> "lns.yaml is valid."
lns inspect              # render the effective definition (merged config, resolved values)
```

`lns inspect` also takes a path (`.`, `lns.yaml`, `./dir`) to render another
directory's definition, still offline; a run id or registry reference inspects
live or cached state instead — see [`lns sandbox`](cli-reference.md#lns-sandbox).

`validate` reports each problem with its cause and exits non-zero when the
definition is broken:

```text
lns.yaml is not valid:
  - schema: sandbox must carry an image; it is the base OCI image the sandbox runs
```

## `lns run`

```bash
lns run [OPTIONS] [REF] [-- COMMAND...]
```

`REF` is a sandbox reference: a **registry coordinate** (`ghcr.io/team/hermes:1.4.0`)
or a **path to a local definition**. Omit it to run the `./lns.yaml` in the current
directory; `.`, `lns.yaml`, and `./lns.yaml` mean the same thing, a relative or
absolute path runs another directory's definition, and a path-shaped `.yaml`/`.yml`
file names the definition itself. Either way the definition's relative binds and
filesets root at its own directory, compose-style, while the policy still comes
from where you run:

```bash
lns run                                  # run ./lns.yaml in this directory
lns run .                                # same
lns run ../other-project                 # run that directory's lns.yaml
lns run ./docs/examples/claude-code      # same, no cd needed
lns run ./lns.dev.yaml                   # run a definition file by name
lns run ghcr.io/acme/agent:latest        # run a published sandbox by reference
```

You run `lns run` from a project directory; that's where Lens Sandbox looks for the
`lns-policy.yaml` that governs the run. To expose your actual host files to the
workload, bind-mount a directory with `-v /host/path:/guest/path` (see
[Host bind mounts](#host-bind-mounts)); for scratch space that persists across runs,
attach a named volume instead.

### Running a definition vs. a reference

The primary workflow is definition-first — `init`, edit `./lns.yaml`, `run`, then
`push` to share:

```bash
cd ~/dev/my-app
lns init                                 # scaffold ./lns.yaml
$EDITOR lns.yaml                         # set spec.image and the fields you need
lns run                                  # run it
```

To run a sandbox somebody else published, name its registry reference directly:

```bash
lns run ghcr.io/acme/agent:latest
```

The sandbox artifact and its base image are pulled and cached by the service, then
its entrypoint starts inside the microVM.

### Selecting a definition file

`./lns.yaml` is the sandbox — one directory is one sandbox, and every verb
defaults to it. When a project keeps a variant of its sandbox alongside the
default (say `lns.dev.yaml` with looser resources), select it explicitly with
`-f`/`--file` — the same escape hatch as `docker build -f`:

```bash
lns run -f lns.dev.yaml                  # run the variant
lns sandbox validate -f lns.dev.yaml     # validate it, offline
lns push -f lns.dev.yaml ghcr.io/acme/agent-dev:1.0.0
lns inspect -f lns.dev.yaml              # render it, offline
```

The selected file's directory roots its relative binds and filesets, exactly as
a path-shaped `REF` does, and the policy still comes from where you run. On
`lns run` the selector is exclusive with `REF`; `lns run ./lns.dev.yaml` is the
equivalent path spelling.

### Pinning a published sandbox by digest

A tag like `:latest` is mutable — the registry can republish it at any time. To run
exactly the published sandbox you vetted, pin its reference by digest:

```bash
lns run ghcr.io/acme/agent@sha256:8a2c…
```

Pinned manifests are cached, so reruns of a pinned reference don't re-consult the
registry. `lns pull` pre-warms the cache ahead of a run.

### Overriding the command

A command after the reference replaces the sandbox base image's default command
while keeping its `ENTRYPOINT`; an explicit `--` separator is still accepted:

```bash
lns run ghcr.io/acme/agent:1.0.0 sh -c 'echo hello && uname -a'
lns run ghcr.io/acme/agent:1.0.0 -- sh -c 'echo hello && uname -a'
```

To replace the image's `ENTRYPOINT` as well, pass `--entrypoint`; the command after
the reference becomes its arguments (`--entrypoint ""` clears the entrypoint
entirely):

```bash
lns run --entrypoint /bin/sh ghcr.io/acme/agent:1.0.0 -c 'echo hi'
```

### A command with no reference

`lns run -- <command>` (or `lns run <command>` for an unambiguous word) runs the
`./lns.yaml` definition with its command overridden — there is no imageless mode;
a run always boots a sandbox definition or a published reference.

### Setting the run user and hostname

`-u`/`--user` picks the run-as user or uid (`USER[:GROUP]`, Docker-style; a numeric
segment is used as the uid), and `-h`/`--hostname` sets the guest hostname:

```bash
lns run -u 1000:1000 -h build-box ghcr.io/acme/agent:1.0.0
```

A definition can ask for its own run-as user, so it does not need a wrapper to
pass `-u`:

```yaml
spec:
  user: root                # or `node`, `node:staff`, `1000`
```

The order is: `-u` on the command line, then `spec.user`, then the image's
`USER`, then the unprivileged `sandbox` user (uid 65534). A **name** is resolved
from the guest's own passwd file, so `user: node` gets whatever uid that image
gives `node`; a **number** is used as the uid directly. `lns inspect` shows the
declared user, including for a pulled sandbox, so an artifact that wants root is
visible before it boots.

`HOME` and `USER` come from that user's guest passwd entry, so `-u root` normally
gives the workload `HOME=/root`. A definition's `env:` or a `-e` on the command
line overrides both — what you declare wins, and nothing rewrites it behind you.
An image's own `ENV HOME` does not: an image shipping `ENV HOME=/root` must never
hand an unprivileged workload a home it cannot write.

```yaml
spec:
  env:
    HOME: /home/sandbox     # kept, even under `lns run -u root`
```

### Resources

| Flag                          | Default | Meaning            |
| ----------------------------- | ------- | ------------------ |
| `--cpus <N>`                  | `1`     | Number of vCPUs (at least 1). |
| `-m`, `--mem`, `--memory <SIZE>` | `512` | RAM in mebibytes, or with a unit suffix. |

Memory accepts Docker-style unit suffixes (`b`, `k`, `m`, `g`, plus `kb`/`kib`
and friends), rounded up to a whole MiB. The flag reads the same sizes that
`spec.resources.memory` writes, so `512Mi` and `38Gi` work in both places:

```bash
lns run --cpus 4 -m 2g ghcr.io/acme/builder
lns run --mem 2048 ghcr.io/acme/builder        # same thing, in MiB
lns run -m 38Gi ghcr.io/acme/builder           # the unit a definition uses
```

Every suffix is binary: `2g`, `2gb`, `2Gi` and `2GiB` all mean 2048 MiB.

A definition can also size itself as a **share of the host**, so a published
sandbox is sensible on whatever machine runs it:

```yaml
spec:
  resources:
    cpu: 80%
    memory: 80%
```

The share is of the host's **total** cores and RAM, not what happens to be free,
so the same definition sizes identically on every run of one machine and
`lns inspect` records a size that reproduces. A share is a request, so it is
lifted to the built-in 1 vCPU / 512 MiB when the arithmetic lands below that —
a small machine still boots. It is still capped by the same ceiling an absolute
size is, so `100%` on a very large host cannot starve it. When the host reading
is unavailable the share is ignored and the built-in default applies, with a
warning naming the field.

### Working directory

Set a portable working directory in `lns.yaml`, or override it for one run with
`-w`. The precedence is explicit `--workdir`, then `spec.workdir`, then the
image's `WORKDIR`. Every form must be an absolute guest path, and the directory
is created inside the sandbox when it is missing.

```yaml
spec:
  workdir: /workspace
```

```bash
lns run -w /workspace ghcr.io/acme/agent
```

### Naming a run

Every run has a numeric id (`run #7`) **and** a name. Pass `--name` to choose
one; omit it and Lens Sandbox assigns a memorable `adjective_noun` name. Either
handle works anywhere a run id is accepted:

```bash
lns run -d --name reviewer ghcr.io/acme/agent
lns logs reviewer
lns stop reviewer
```

A name may contain letters, digits, `_`, `.`, and `-`, and must not be all
digits (so it is never mistaken for an id). Names are unique among the runs
`lns ps` shows, and free up once the run is removed.

### Persistent defaults

The resource gap-fillers you'd otherwise repeat on every run can be stored once
with [`lns config`](cli-reference.md#lns-config). A per-run flag always overrides
its configured default:

```bash
lns config set run.cpus 4
lns config set run.mem 2048
lns run ghcr.io/acme/builder            # boots with 4 vCPUs · 2048 MiB
lns run --cpus 2 ghcr.io/acme/builder   # per-run flag wins: 2 vCPUs
```

The settable defaults are `run.cpus`, `run.mem`, and `run.registry`. Environment
variables, volumes, and ports are properties of the sandbox itself — set them per
run with the flags below, or bake the ones that always apply into the sandbox
definition's `spec` (`env`, and so on).

### Environment variables

Set non-secret environment variables with `-e KEY=VALUE` (repeatable):

```bash
lns run -e NODE_ENV=production -e LOG_LEVEL=debug ghcr.io/acme/agent
```

Load them in bulk from a file with `--env-file` (repeatable):

```bash
lns run --env-file app.env ghcr.io/acme/agent
```

An env file holds one `KEY=VALUE` per line; blank lines and `#` comments are
skipped. When the same variable appears more than once, later files win and
`-e` beats every file. A bare `KEY` line with no `=` is rejected — passing
host variables through implicitly would be a silent leak channel. Environment that
should always apply belongs in the sandbox definition's `spec.env`.

Secrets do **not** belong in either flag — `-e` and `--env-file` values are
plain environment variables visible to the workload. Use the
[credential flow](credentials.md) so real secrets stay outside the sandbox.

### Volumes

Named volumes persist data across runs, Docker-style:

```bash
lns run -v build-cache:/root/.cache ghcr.io/acme/builder
lns run -v config:/etc/app:ro ghcr.io/acme/agent      # read-only
```

The format is `name:/absolute/path[:ro]`. The volume `name` may contain letters,
digits, `_`, `.`, and `-`; the target must be an absolute path with no `.`/`..`
segments. Volume contents are stored by the service and survive between runs.

A volume outlives the identity that wrote it, so its **root directory** is
transferred to the run-as user on each writable attach. That is what lets a
volume first written under one `-u` still be written under another. Files already
inside it keep the owner they had — a volume written as root and then attached
unprivileged still holds root-owned files, and a tool that checks ownership (git's
`safe.directory`, for one) will say so.

Manage the store with `lns volume`:

```bash
lns volume ls                  # list volumes: on-disk size, age, holding run
lns volume create build-cache  # provision a volume before its first run
lns volume inspect build-cache # full details as JSON
lns volume rm build-cache      # delete one volume (refused while a run holds it)
lns volume prune               # delete every volume no running sandbox holds
```

`rm` and `prune` never touch a volume that a live run has attached, and `prune`
asks for confirmation unless you pass `-f`/`--force`. Everything else in a
sandbox is ephemeral by design — volumes are the one place data persists, so
removing one is permanent.

A run that finishes, or that you `lns stop`, **releases** each volume before the
guest powers off, so the image is left marked clean. A run the host cannot ask to
stop — `lns kill`, or a service killed outright — cannot do that, and leaves its
volumes dirty.

A volume image carries an internal journal, so an interrupted run leaves it
mountable and consistent — the kernel replays the journal at the next attach. A
volume created by an older build has no journal, and gains one only when you
recreate it (`lns volume rm`, then the next run). Every volume also mounts with
`errors=remount-ro`, so an image the kernel does find inconsistent stops taking
writes instead of compounding the damage.

Once an image has been left dirty, it stays dirty. The kernel restores the state
it found at mount, so a later clean run does not clear the mark, and there is no
in-guest `e2fsck` to clear it either — only recreating the volume does.

### Declarative mounts

Mounts that are part of the sandbox belong in `spec.volumes`. A named volume
uses `type: volume`; a host bind uses `type: bind`:

```yaml
spec:
  volumes:
    - type: bind
      source: .
      target: /workspace
    - type: volume
      source: agent-config
      target: /home/node/.config/agent
      readOnly: true
```

`target` is always an absolute guest path. A named-volume `source` follows the
same naming rules as `lns run -v name:/path`. The older
`{name: agent-config, target: /path, readOnly: true}` shape remains accepted.
Duplicate targets in one `lns.yaml` are rejected.

A relative bind source is resolved from the directory containing the local
`lns.yaml`. For a published sandbox it is resolved from the directory where the
consumer invokes `lns run`; this keeps `source: .` aligned with one directory =
one project. Sources are paths, not shell expressions: Lens Sandbox performs no
shell or environment-variable interpolation, so use `source: .`, not `$PWD`.
Paths are normalized, and a relative source cannot escape the project with
`..`.

A source that starts with `~/` resolves against the home directory of the user
who runs `lns`, on the machine that runs it. This is the portable way for a
definition to reach a per-user directory such as `~/.claude`. Only the `~/` form
is supported: a bare `~` and another user's home (`~alice/…`) are refused,
because a definition that names one machine's account is not portable.

A `~/` source that names a secret store — `~/.ssh`, `~/.aws`, `~/.gnupg`, or any
other secret-shaped segment — is refused offline, exactly as a `hostPath` is.
The keep/drop prompt reads the top-level names under a bind root only, so it
cannot be the guard for a bind whose root *is* the key store. An absolute or
project-relative source keeps the rules it always had, because it names the
author's own machine and cannot travel.

A bind whose source is absent refuses the run and names the path. Add
`optional: true` to skip it instead, so one definition runs both on a machine
that has the directory and on a machine that does not:

```yaml
spec:
  volumes:
    - type: bind
      source: ~/.claude            # this machine's own agent state
      target: /home/agent/.claude
      optional: true               # skipped, with one line, when it is absent
```

A skipped bind reaches nothing downstream — not the secret scan, not the guest,
not the audit record. `optional` applies to a bind only; a named volume is
created on demand and is never absent.

Launch flags are the final override layer. Lens Sandbox starts with the mounts
from `lns.yaml`, replaces a declarative mount when an explicit `--volume` or
`--mount` targets the same guest path, and keeps mounts with other targets.

### Filesets — files shipped inside the artifact

`spec.volumes` attaches the *consumer's* files: live host binds and named
volumes on the machine where the sandbox runs. `spec.filesets` is the opposite
direction — files the *author* ships **inside** the published artifact (agent
settings, skills, prompt libraries, seeded tool state), materialized into the
guest at launch as a snapshot:

```yaml
spec:
  filesets:
    - path: ./seed                # a directory in the author's project
      mountPath: /home/sandbox    # owned by the workload user (the default)
    - ref: ghcr.io/team/settings@sha256:…   # a pre-published FileSet artifact
      mountPath: /root/.agent/settings
      owner: root                 # pinned input: the workload can't rewrite it
    - inline:                     # small text files kept in lns.yaml itself
        .claude/settings.json: |
          {"permissions":{"defaultMode":"bypassPermissions"}}
        mcp.json: |
          {"mcpServers":{}}
      mountPath: /home/sandbox
    - hostPath: ~/.gitconfig      # one file from the machine that runs it
      mountPath: /home/agent/.gitconfig
      optional: true
```

- **`path`** names a directory in the authoring project. A local `lns run`
  snapshots it at launch — the guest sees exactly what a consumer of the
  published artifact would see (live files are `spec.volumes`' job). At
  `lns push`, each `path` directory is packed into a deterministic FileSet
  artifact, uploaded alongside the sandbox, and rewritten to a digest-pinned
  `ref` in the published config. (`spec.image` is published exactly as written —
  `lns push` does not resolve it; pin it by digest yourself for a reproducible
  base, as above.) A `path` is a directory beside the definition, so it cannot
  be home-anchored: a `~/…` `path` is refused and names `hostPath` as the field
  that reads the machine that runs the sandbox.
- **`ref`** names a pre-published FileSet artifact, always pinned by digest. A
  pulled sandbox's filesets are fetched and materialized at launch; `lns
  inspect` lists every fileset (`fileset: <ref> -> <mountPath>`) so you can
  review what a sandbox ships before running it, and the run summary
  discloses them as `Fileset:` lines.
- **`inline`** maps safe relative file paths to UTF-8 text. It is useful for
  small settings such as `mcp.json` or agent configuration when the definition
  should stay self-contained. Each file is limited to 128 KiB after YAML
  parsing, an inline fileset totals at most 1 MiB across at most 256 files.
  Inline files remain in the published sandbox config, so `lns push`
  does not create a companion FileSet artifact for them. Inspect and run output
  disclose the inline source, mount path, and owner, never the file contents.
- **`hostPath`** names one file on the machine that *runs* the sandbox, not on
  the author's. It is read once at launch and written to `mountPath`, so the
  guest gets a snapshot, never a live share — the file a workload edits inside
  the guest dies with the microVM, and the host copy is never touched. This is
  the portable way to seed the tool identity a workload needs, such as
  `~/.gitconfig`. The path must be absolute or start with `~/` (resolved
  against the home of the user who runs `lns`), and `mountPath` names a guest
  **file**, so it must not end in `/`. A `hostPath` is carried into the
  published config verbatim — `lns push` packs nothing for it, which is exactly
  what keeps the artifact portable. Add `optional: true` when the file may be
  absent: an absent required `hostPath` refuses the run and names the path, an
  absent optional one is skipped and reported as one status line of the run.
  A `hostPath` that is a symlink reads the file it points at, so a dotfile that
  stow, chezmoi, or home-manager manages works; a link that points nowhere
  counts as absent. `optional` applies to a `hostPath` entry only.
- Each entry sets exactly one of `path`/`ref`/`inline`/`hostPath`. `inline` must
  contain at least one file. Every inline key must be a relative path without empty, `.`,
  or `..` components. `mountPath` is an absolute
  guest path; duplicates — including collisions with a volume `target` — are
  rejected offline, as is any mount into the sandbox's own `/.lens` runtime
  namespace.
- **`owner`** decides who owns the materialized files in the guest.
  The default, `workload`, transfers the mount path and everything it ships
  to the run-as user, so a seeded config the tool rewrites at runtime
  (`~/.claude.json`-style state) just works. `owner: root` pins the content
  beyond the workload's reach — the right choice for skills, prompts, and
  MCP configs an agent must not rewrite mid-run. Either way the snapshot is
  ephemeral: changes die with the microVM.
- A secret-shaped file (`.env`, keys, credential stores) anywhere in a `path`
  fileset, as any path component in an `inline`
  fileset, or as any segment of a `hostPath`
  refuses `validate`, `run`, and `push` outright: a fileset is baked
  into an artifact, so there is no keep/drop prompt to catch it later — real
  secrets stay outside the workload. Ship the tool's *configuration* in a
  fileset and bind its *credential* through `spec.credentials`, so the
  published artifact stays secret-free on every machine that pulls it.

The trust model is pinning plus disclosure: a published sandbox whose fileset
ref is not digest-pinned is refused, and what a sandbox ships is always
visible in `lns inspect` before anything runs.

### Mixins — what a sandbox layers on

`spec.mixins` names documents this sandbox builds on. Each one is merged before
the run plans anything, and what it contributes is enforced exactly like
something the sandbox wrote itself:

```yaml
spec:
  image: docker.io/library/debian:bookworm-slim
  mixins:
    - ./mixins/postgres-tools
    - ghcr.io/acme/observability@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582
```

A mixin is a document of `kind: mixin`. It may carry anything a sandbox can
except the blocks that describe one launch — `image`, `command`, `workdir`,
`user`, `resources` — and it may not name a connector.

You publish one the same way you publish a sandbox, because it is the same kind
of thing:

```bash
lns push ghcr.io/acme/observability:2 -f observability.yaml
lns inspect ghcr.io/acme/observability:2   # what it carries, as written
lns pull ghcr.io/acme/observability:2      # cache it and the mixins it names
```

**The last source to say something about a thing wins.** Sources are ordered:
the sandbox first, then each entry in `spec.mixins` in order with that mixin's
own mixins expanded right after it, then each `--mixin` the user passed.

A directory entry is read from this machine, relative to the document that names
it. A published sandbox may not name one — a consumer has no copy of your
working directory — so `spec.mixins` in a document you `lns push` must be
digest-pinned. `lns validate` reports a directory that holds no `lns.yaml`.

You can add your own for a single run:

```bash
lns run . --mixin ./mixins/debug-tools
lns run ghcr.io/acme/agent:1 --mixin ghcr.io/acme/observability:2
```

A directory merges only into a document this machine read, whoever names it — so
`--mixin ./dir` works on a local run and is refused for a published sandbox.

A `--mixin` may be a tag, where a document's entry may not. The run pins it
before it reports it, so the summary names the exact bytes you approved:

```
  Mixins:    /work/mixins/debug-tools, observability:2 → ghcr.io/acme/observability@sha256:c41e8b7d20a9…
```

A composed run is a document you did not write in full, so the summary says
where each line came from — and lists the rules and credentials the merge
produced, which an uncomposed run has no second author to attribute:

```
  Volume:    cache → /home/agent/.cache  [from /work/mixins/debug-tools]
  Tools:     node@22  [from ghcr.io/acme/observability@sha256:c41e8b7d20a9…, replaced node@20 from the sandbox]
  Rules:     allow api.vendor.example  [from ghcr.io/acme/observability@sha256:c41e8b7d20a9…]
             deny proxy.vendor.example  [from the sandbox]
  Credentials: SOME_TOKEN  [from ghcr.io/acme/observability@sha256:c41e8b7d20a9…]
```

A run that resolved no mixin prints what it always has: one author, nothing to
attribute.

`lns inspect <REF> --mixin <REF>` shows the same composition without starting a
run.

### Tools — declared toolchains

`spec.tools` names the developer tools the workload needs, without baking them
into the base image or reinstalling them over the network on every cold start:

```yaml
spec:
  image: docker.io/library/debian:bookworm-slim
  tools:
    - node@22
    - python@3.12
```

- Each entry is a portable `name@version`. A version is required — `node@22`
  pins a major line, `node@latest` says so explicitly, and a bare `node` is a
  validation error. Engine syntax (`aqua:`, `ubi:`, `npm:` prefixes) never
  appears in a definition.
- A bounded line resolves **once per machine** and stays there: the first run of
  `node@22` here records the exact version it picked (say 22.11.0) and every
  later run reuses it, even after upstream publishes 22.12.0. That is what makes
  a warm run reproducible and offline-capable. `node@latest` is the opposite by
  definition — it re-checks the version index on every run, so a new release is
  picked up the next time you start. To move a bounded line, edit the version you
  declared.
- `lns sandbox validate` checks the shape offline; the version resolves when
  the tools are provisioned.
- The service provisions declared tools **before the microVM boots** and caches
  them per machine: the first run of a tool set downloads it, and every later
  run — including rebuilt microVMs — reuses the cache without touching the
  network. The one exception is a `@latest` entry, which asks the version index
  each run; when the index is unreachable it falls back to the last version
  resolved here, so an offline start still works. In a local `lns.yaml`,
  declaring the tool is the consent. A published sandbox instead discloses its
  tool installers and asks the consumer before provisioning them; `--yes`
  accepts them in a non-interactive run or pull. What the tools *do* at runtime
  (npm, pip, go traffic) stays inside the normal policy cage.
- **Provisioning runs the tool's own install code, not just a download.** It
  happens in a disposable guest of its own — separate from your workload, thrown
  away when the install finishes, and never sharing your workload's writable
  layer or your project's bind mounts. That guest reaches the network freely:
  tool backends are arbitrary upstream hosts, so no allowlist would fit, and it
  runs no policy gate. Every declared tool installs in that one guest, as root,
  so a tool you declare is trusted by the tools declared alongside it. The cache
  keeps that boundary: a tree records which tools shared its guest, and another
  sandbox reuses it only if it declares those tools too — so a published sandbox
  you don't trust cannot hand you a poisoned `node@22`. This is still a stronger
  trust assumption than pulling `spec.image`, which only fetches inert layers:
  declare tools you trust as you would a base image you run.
- On a **musl** image, declaring `node` or `bun` also injects the pinned Alpine
  `libstdc++`/`libgcc` those builds link, at their canonical `/usr/lib` paths. If
  your image ships its own newer copies the injected ones replace them for that
  run, and the image's own C++ binaries can then fail with a `GLIBCXX_…` version
  error; use a glibc base image if that matters.
- The per-machine tool cache is **not reclaimed automatically**. Every version
  a tool resolves to keeps its tree, so a long-lived `node@latest` accumulates
  one per upstream release. `lns sandbox prune --force` reclaims the provisioned
  tool cache when no sandbox is running; while any run is live it keeps the
  shared tool content intact. The next run re-provisions what it needs — to the
  same versions, because prune reclaims the cached trees without touching what
  this machine already resolved.
- Tools land read-only on the workload's `PATH`, ahead of the base image's own
  copies. One caveat: a **login** shell (`sh -lc`, `bash -lc`) sources
  `/etc/profile`, which on most images resets `PATH` outright and so discards the
  tool dirs — run your command without `-l`, or re-export `PATH` yourself.
  Nothing tool-related persists in the workload's writable layer — the
  per-machine tool cache is a host-side input, not guest state, so a workload
  that shadows a tool in its own overlay affects only that run and never the
  cache other projects read.
- Declared tools are always disclosed: `lns inspect` lists each entry, and the
  run summary shows them at launch. Provisioning is recorded in the
  [audit chain](audit.md) — in the run's own chain when a run does the
  fetching, and on the machine-level chain when `lns pull` provisions a
  published sandbox's tools before its first run.

Tools stack on top of `spec.image` — the image stays required and still decides
the OS userland; matching builds are selected for its libc flavor (musl or
glibc). A tool with no build for the image's flavor refuses the launch with
both remedies: switch to a glibc base image, or bring the runtime via
`spec.image` as before.

### Host bind mounts

When the source of a `-v` is an **absolute host path** rather than a name, it's a
host bind: the workload sees your live host files at the target, Docker-style.

```bash
lns run -v "$(pwd)":/work ghcr.io/acme/agent        # the agent edits your repo
lns run -v /etc/myapp:/config:ro ghcr.io/acme/app   # read-only
```

The format is `/host/path:/absolute/target[:ro]`. The source must be an absolute
path that already exists (a missing path is refused, not silently created — the one
deliberate divergence from `docker run`) and it must be a **directory**. To give the
workload a single host file, bind the directory that holds it. Binds default to
read-write; append `:ro` for read-only. Disambiguation is by shape: a leading `/` is a host bind, anything
else is a named volume, so `-v build-cache:/cache` is still a volume.

#### Secrets in a bind

A bind exposes everything in the directory — including `.env` files, SSH keys, and
other credentials that the [credential flow](credentials.md) is designed to keep
*outside* the workload. Before the run starts, `lns` scans the **top level** of each
bind for secret-shaped files (`.env*`, `*.pem`, `*.key`, `id_rsa`, `.npmrc`,
`.netrc`, `.ssh/`, `.aws/`, …) and asks you, once per file, whether to **keep** it
(expose the real file) or **drop** it (hide it from the workload). Your choice is
remembered per-machine, so later runs don't ask again:

```text
Host bind: /Users/you/proj/.env looks like a secret. Expose it to the workload? [k]eep / [D]rop (default):
```

- **Keep** mounts the real file through; **Drop** masks it so the workload can't
  read it (the file is never modified or deleted on the host).
- The default on a bare Enter is **drop** — the safe choice.
- A non-interactive run (`-d`, or no terminal) drops any undecided secret and notes
  it on stderr, rather than exposing it unasked.
- Decisions to **keep** a real secret are per-machine and never written to a shared
  file. To share a "never expose these" rule with your team, commit a `.lensignore`
  in the bind root — one path per line — and those paths are dropped with no prompt.
  An entry may be a top-level name or a nested path relative to the bind root
  (`packages/api/.env`); it must stay inside the bind (no leading `/`, no `..`), and a
  rule for a file that isn't present is simply a no-op.

The run summary lists each bind, its mode, and the disposition of every detected
secret (`kept (exposed)` / `dropped`).

#### Excluding subpaths from a bind

Binding a directory does not force you to expose all of it. A bind volume takes an
`exclude` list of paths relative to its root, so a definition can share a parent
directory while holding back the parts a workload must not see:

```yaml
spec:
  volumes:
    - type: bind
      source: ~/dev              # or an absolute path
      target: /work
      exclude:
        - .cargo          # host toolchains the guest must not use
        - .rustup
        - lns-sandboxes   # other sandboxes' state, sockets included
```

Directories and nested paths both work, an entry must stay inside the bind (no
leading `/`, no `..`), and an entry for a path that isn't there is a no-op. There is
no prompt: an exclude is the author's rule, not a per-machine decision, so it is
never written to the KEEP/DROP store. A `.lensignore` in the bind root does the same
job for a rule you don't want in the definition; naming a path in both drops it once.

The semantic is **masked, not absent**. An excluded directory appears in the guest as
an existing, empty, unwritable directory, and an excluded file reads as empty rather
than missing. A tool that probes for `~/.cargo` will find it — and find nothing in
it. virtio-fs shares the whole tree, so masking is the lever available; if a workload
must not know a path exists at all, bind a narrower source instead.

Declarative binds use this exact scan, prompt, remembered KEEP/DROP decision,
masking, validation, and audit path. A published sandbox is inspected and pinned
before launch so its bind declarations can be shown and approved on the consumer
host; publishing a sandbox never grants it silent access to host files.

> **The automatic scan is top-level only.** Only the immediate contents of the bind
> root are scanned for secret shapes, so a secret nested in a subdirectory
> (`packages/api/.env`, a key under `server/certs/`, credentials embedded in
> `.git/config`) is exposed to the workload **without a prompt**. To hide a nested
> secret you know about, name it in `.lensignore` (a nested path is honored); for an
> untrusted subtree, bind a narrower path or use `:ro`.

### Publishing ports

Expose a guest port on the host with `-p` (repeatable):

```bash
lns run -p 3000:3000 ghcr.io/acme/web            # 127.0.0.1:3000 -> guest 3000
lns run -p 8080:80 ghcr.io/acme/web              # remap host 8080 -> guest 80
lns run -p 0.0.0.0:3000:3000 ghcr.io/acme/web    # expose beyond this machine
```

The format is `[host_ip:]hostport:containerport[/proto]`:

- The host bind defaults to loopback (`127.0.0.1`), so a published port is only
  reachable from your machine unless you pass an explicit `host_ip`. The run
  summary flags a non-loopback bind as `(exposed beyond this machine)`.
- IPv6 host IPs go in brackets: `[::1]:8080:3000`.
- Only `tcp` is supported today; `udp` is rejected.

Declared `spec.ports` follow the docker family on both sides of the trust line:

- **Your own `./lns.yaml` is `docker compose up`**: every declared port
  publishes automatically — the `host:` value when present, the container
  number otherwise. The definition in your directory is policy you wrote;
  the run summary's `Ports:` line discloses each binding.
- **A pulled sandbox is `docker run`**: its declared ports are EXPOSE-style
  disclosure, not a grant — a bare run publishes nothing, and the summary
  lists them as declared but not published. Opt in to the declared set with
  `-P`/`--publish-declared` (same mapping). A pulled artifact's definition is
  policy you run into, so inbound access stays yours to grant.
- Declared publishing always binds loopback — compose's `0.0.0.0` default is
  deliberately not copied. To expose wider, name the port explicitly with
  `-p host_ip:hostport:containerport`.
- Explicit `-p` entries combine with the declared set; on a container-port
  conflict the explicit `-p` wins.

### Interactive, TTY, and detached sessions

| Flag                  | Default | Meaning                                                              |
| --------------------- | ------- | -------------------------------------------------------------------- |
| `-i`, `--interactive` | `true`  | Keep stdin open and forward host stdin to the workload. Disable with `--interactive=false` (or `-i=false`). |
| `-t`, `--tty`         | `true`  | Allocate a PTY. Pipe mode is selected automatically when stdin isn't a TTY. Disable with `--tty=false` (or `-t=false`). |
| `-d`, `--detach`      | `false` | Start the run and return immediately; it keeps running in the service.|

By default a run is attached to your terminal. Detach to leave it running in the
background:

```bash
lns run -d ghcr.io/acme/long-job
# prints: run #7
```

A detached run is reachable later via the
[running-sandbox lifecycle verbs](#managing-running-sandboxes).
`-d` cannot be combined with `-i`/`-t`.

### Detaching from an attached run

While attached, the detach chord (default `ctrl-p,ctrl-q`) is a docker-style
detach: `lns` returns `0` and the run keeps executing in the background — no
signal is sent to the workload. Re-join it any time with
[`lns attach`](#attaching), and detach again with the same chord. Change the chord
with `--detach-keys`:

```bash
lns run --detach-keys ctrl-x,ctrl-x ghcr.io/acme/agent
```

The value is a comma-separated chord of single characters or `ctrl-X` tokens.

A deliberate detach is distinct from the `lns` process dying unexpectedly: if the
CLI is killed (or its connection drops) without the chord, the attached run is
cancelled. Use the chord — or start the run with `-d` — to step away safely.

## Distributing a sandbox

Publishing is one step. `lns push` (a shortcut for `lns sandbox push`) builds
`./lns.yaml` and uploads it to a registry as a sandbox artifact:

```bash
lns push ghcr.io/acme/reviewer:1.0.0
```

A reference that names no registry (`lns push acme/reviewer:1.0.0`) resolves
against the Lens hub, `hub.lns.run` — set `run.registry` (or pass `--registry`
on `lns run`) to point bare references somewhere else:

```bash
lns config set run.registry ghcr.io
```

`lns push --dry-run` does everything short of uploading — validates the
definition, packs the filesets, builds the artifact — and prints the digests
that would publish (`npm publish --dry-run`-style), so you can preview a
release offline.

`lns push` also resolves each fuzzy `spec.tools` version (`node@22`,
`node@latest`) against the tool's public version index and embeds the exact pin
in the published definition — the same publish-time pinning filesets get, so
consumers run exactly what you tested. `--dry-run` stays offline and does not
resolve; it notes when declared tools mean the published digest may differ from
the preview.

Pushing needs a stored login with **push access** for the registry — sign in once
with `lns login`. For `ghcr.io` that's a GitHub token with the `write:packages`
scope, pushed to a repository path you own; the GitHub CLI mints one directly:

```bash
gh auth refresh --scopes write:packages
gh auth token | lns login ghcr.io --username <YOUR-GITHUB-USER> --password-stdin
```

For any other registry, pipe a push-scoped token the same way:

```bash
echo "$TOKEN" | lns login registry.example.com --username <USER> --password-stdin
```

A refused push says which of the two it is — no stored login, or a stored login
the registry rejected (expired, missing push scope, or the wrong repository
path) — and prints the sign-in recipe to fix it.

On the other side, `lns pull` fetches a published sandbox and its base image into
the local cache, and `lns run` can boot it straight from the reference:

```bash
lns pull ghcr.io/acme/reviewer:1.0.0     # pre-warm the cache
lns run  ghcr.io/acme/reviewer:1.0.0     # run it
```

`lns push` preserves `workdir` and every mount declaration in the artifact.
Consumers resolve relative binds against their own project directory, not the
publisher's. Preflight pins the resolved artifact digest; after `lns pull` has
cached that artifact and its referenced OCI content, and the service has
provisioned its declared tools once, the published sandbox can start offline
from the cached snapshot. If the artifact declares tools, `lns pull` prints
them and asks before running their installers in the disposable provisioning
guest. A non-interactive pull must pass `--yes`; approval applies to the digest
shown by preflight, so a tag that changes before the pull is refused.

Re-reference a cached sandbox under another tag with `lns tag` (docker-tag style):

```bash
lns tag ghcr.io/acme/reviewer:1.0.0 ghcr.io/acme/reviewer:stable
```

## Managing running sandboxes

Everything you do to a run after starting it is a top-level docker-style verb, each
an exact shortcut into `lns sandbox`:

```bash
lns ps                     # list running sandboxes with their CPU and memory
lns exec 7 -- bash         # open another session inside a run
lns kill 7                 # send one signal (default SIGTERM)
lns stop 7                 # SIGTERM, wait up to 10s, then SIGKILL
lns stop 7 -t 30           # give it longer to clean up
lns logs 7                 # print the captured output so far
lns logs -f 7              # ...and keep following until it exits
lns attach 7               # re-join a detached run live
lns inspect 7              # state + launch config as JSON
```

The full namespace holds the same verbs plus the lns-native `ls` — the
alias-bearing list of **cached** sandboxes (`lns sandbox ls` / `lns sandbox list`),
the counterpart to `lns ps`'s list of running ones:

```bash
lns sandbox ls             # list cached sandboxes (pulled or built)
lns sandbox exec 7 -- bash # (identical to `lns exec 7 -- bash`)
```

Every verb takes a run's **name** as readily as its numeric id — `lns stop
reviewer` and `lns stop 7` are equivalent. `exec` is also reachable as the bare
`lns exec`.

### Exec — another command inside a run

`lns exec 7 -- ls /workspace` runs one command in a second session, like
`docker exec`. The run id is shown by `lns run -d` and `lns ps`. `--detach-keys`
works as it does for `lns run`; detaching from an exec session closes only that
session — the run and any other sessions keep going.

The session is **non-interactive**: `-i` and `-t` are refused, because an exec
session has no way yet to route your keystrokes to itself rather than to the run's
own workload. So `lns exec 7 -- bash` gets you a shell with closed stdin, which
exits at once. To reach a live terminal inside the run, use `lns attach`.

An exec **joins the run it names**, so a diagnostic command sees the same sandbox
the workload does: the run's resolved environment (base image `ENV`, `spec.env`,
`-e`, and its declared tools on `PATH`), its working directory, the CA bundle its
runtimes trust, and the credential placeholders — so a request you make from an
exec goes through the same policy and the same credential swap. You never have to
re-export the environment by hand, and `~` means what it means to the workload.

The supervisor's own handshake variables stay out, so a piped exec is not told it
has a colour terminal. Real credential values stay out too: an exec is handed the
same placeholder the workload holds, never the secret behind it.

### Stopping vs killing

`lns kill` sends one signal (case-insensitive, bare or `SIG`-prefixed: `TERM`,
`INT`, `QUIT`, `HUP`, `WINCH`, `KILL`) and returns. `lns stop` owns the whole
shutdown: it sends `SIGTERM`, waits up to the timeout for the workload to exit, and
only then sends `SIGKILL`. The command reports which of the two happened —
`stopped run #7` for a graceful exit, `killed run #7` when it had to escalate.

### Logs

The service keeps a rolling capture of every run's stdout and stderr — the most
recent 2 MiB — for as long as the run is listed. `lns logs` prints what's buffered;
`-f` streams new output until the run exits. Output of exec sessions is not
captured, only the run's primary session.

### Attaching

`lns attach` joins a run's output from now on (no history replay) and forwards your
keystrokes when the run was started with stdin open. The detach chord (default
`ctrl-p,ctrl-q`) leaves the run running and returns you to your shell —
docker-attach style, no signal is sent — so it's safe to step away from a `-d` run
you want to keep alive. A run started with `-d` has stdin closed, so attach is
primarily a live view of its output.

### Inspecting

`lns inspect <target>` reads whichever kind of sandbox the target names. For a
**running** run it prints one JSON document with the run's status, image, command,
and launch configuration (cpus, memory, env, ports, volumes, run-as identity), plus
the contents of its policy file when that file is readable on this machine. For a
**cached** reference it prints the artifact's kind and definition — a plain `image`,
or a `sandbox`'s image, workdir, mounts, declared ports, filesets, and connectors,
flagging a permissive default policy.

### Listing resource use

`lns ps` lists running sandboxes with their CPU share and memory — the microVM is
the workload, so the numbers cover everything the run is doing.

A run whose guest has stopped answering — one you just killed, for instance —
still gets a row, with `-` in place of its CPU and memory (`null` under
`--format json`). One unresponsive guest never costs you the rest of the listing.

## Cleaning up the cache

Cached sandboxes accumulate as you pull and build. Remove one with `lns rm` (a
shortcut for `lns sandbox rm`); it frees the layers no remaining cached sandbox
shares and refuses a reference that a run is still using:

```bash
lns rm ghcr.io/acme/reviewer:1.0.0
```

`lns sandbox prune` removes every cached sandbox not held by a running one. When
no sandbox is running, it also reclaims the provisioned tool cache. It requires
`-f`/`--force` — there is no interactive prompt, so nothing is swept until you
ask for it explicitly:

```bash
lns sandbox prune --force
```

Removing a cached sandbox is always safe in the durable sense — the next `lns run`
or `lns pull` simply fetches or rebuilds it again.

## A worked example

[`examples/claude-code/`](examples/claude-code/) is a complete recipe that ties
these pieces together: it runs Claude Code inside a sandbox using `spec.image`
plus node declared under `spec.tools`, `spec.env`, a tight `policy` allowlist,
the `claude-code-subscription` connector, a `.` bind at `/workspace`, and a
self-contained inline fileset that seeds the agent's home. Copy its `lns.yaml`
into your project and `lns run`.

## See also

- [Claude Code example](examples/claude-code/) — a full agent recipe.
- [Policy and approvals](policy.md) — control what the workload may reach.
- [Credentials](credentials.md) — give the workload placeholders, not secrets.
- [CLI reference](cli-reference.md) — the full flag list.
