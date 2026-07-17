# Running workloads

Lens Sandbox has one user-facing noun: the **sandbox**. A sandbox is defined by a
`./lns.yaml` file that pins a base OCI image plus its command, environment, policy,
and integrations. One directory is one sandbox. A sandbox is either **cached**
(pulled or built, sitting in the local cache) or **running**.

You drive it on two tiers:

- The **top level** carries the docker-familiar verbs — `lns run`, `lns ps`,
  `lns stop`, `lns pull`, `lns push`, and so on. Each (except `run`) is an exact
  shortcut into the `lns sandbox` namespace.
- **`lns sandbox <verb>`** is the complete surface. The lns-native verbs that have
  no docker analogue — `validate`, `show`, `ls`, `prune` — live only there.

The background service must be running first (`lns service start`).

## Defining a sandbox

Scaffold a `./lns.yaml` in the current directory with `lns init` (a shortcut for
`lns sandbox init`):

```bash
lns init
```

```yaml
apiVersion: lns.run/v1
kind: Sandbox
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
    defaultVerdict: ask
    allowedRoutes: []
  integrations: []
  credentials: []
  volumes:
    - type: bind
      source: .
      target: /workspace
  filesets: []
  ports: []
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
| `env`          | Non-secret environment variables seeded into the workload.                   |
| `policy`       | The network policy — `defaultVerdict` and `allowedRoutes` (see [Policy](policy.md)). |
| `integrations` | Ids of the [integrations](integrations.md) whose credentials and routes the sandbox needs. Declared ids arm at launch on any machine — no `lns integration connect` step; an id the machine's catalog doesn't know refuses the launch. |
| `credentials`  | Credential slots: each names an integration (`name`), the env var it is injected as (`env`, remapping the catalog default), and optionally `required: true`. A slot arms like a declared integration; a **required** slot with no value bound on the machine refuses the launch before boot, pointing at `lns integration connect` (see [Credentials](credentials.md#value-decisions)). |
| `resources`    | vCPUs and memory the sandbox boots with (`cpu`, `memory` with a unit suffix); per-run `--cpus` / `--mem` flags win. |
| `volumes`      | Named volumes and host binds mounted into the guest; see [Declarative mounts](#declarative-mounts). |
| `filesets`     | Files shipped inside the artifact (`path` packed and digest-pinned at push, or a pre-published digest-pinned `ref`), snapshot-mounted at `mountPath`; see [Filesets](#filesets--files-shipped-inside-the-artifact). |
| `ports`        | Container ports the sandbox serves (`container`, optional `host`), validated offline. Running your own `./lns.yaml` publishes them automatically (compose-style, on loopback); a pulled sandbox's declared ports are disclosure only until you opt in with `-P` — see [Publishing ports](#publishing-ports). |

Check the definition offline — no network, no service — with `validate` and
`show`:

```bash
lns sandbox validate     # schema, cross-field, and secret checks -> "lns.yaml is valid."
lns sandbox show         # render the effective definition (merged config, resolved values)
```

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
or a local **`lns.yaml`**. Omit it to run the `./lns.yaml` in the current
directory:

```bash
lns run                                  # run ./lns.yaml in this directory
lns run ghcr.io/acme/agent:latest        # run a published sandbox / image by reference
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

The quick path skips the file and names a reference directly:

```bash
lns run ghcr.io/acme/agent:latest
```

The image is pulled (and cached by the service) and its entrypoint starts inside
the microVM.

### Pinning an image by digest

A tag like `:latest` is mutable — the registry can republish it at any time. To run
exactly the bytes you vetted, pin the reference by digest, either in `spec.image`
or on the command line:

```bash
lns run ghcr.io/acme/agent@sha256:8a2c…
```

Pinned manifests are cached, so reruns of a pinned reference don't re-consult the
registry. `lns pull` pre-warms the cache ahead of a run.

### Overriding the command

A command after the reference replaces the image's default command (Docker-style)
while keeping its `ENTRYPOINT`; an explicit `--` separator is still accepted:

```bash
lns run alpine:3.20 sh -c 'echo hello && uname -a'
lns run alpine:3.20 -- sh -c 'echo hello && uname -a'
```

To replace the image's `ENTRYPOINT` as well, pass `--entrypoint`; the command after
the reference becomes its arguments (`--entrypoint ""` clears the entrypoint
entirely):

```bash
lns run --entrypoint /bin/sh alpine:3.20 -c 'echo hi'
```

### Imageless runs

Provide a command after `--` with no reference to boot a minimal guest and run it
directly:

```bash
lns run -- /bin/sh
```

(This is distinct from a bare `lns run`, which runs the `./lns.yaml` definition.)

### Setting the run user and hostname

`-u`/`--user` picks the run-as user or uid (`USER[:GROUP]`, Docker-style; a numeric
segment is used as the uid), and `-h`/`--hostname` sets the guest hostname:

```bash
lns run -u 1000:1000 -h build-box alpine:3.20
```

### Resources

| Flag                          | Default | Meaning            |
| ----------------------------- | ------- | ------------------ |
| `--cpus <N>`                  | `1`     | Number of vCPUs (at least 1). |
| `-m`, `--mem`, `--memory <SIZE>` | `512` | RAM in mebibytes, or with a unit suffix. |

Memory accepts Docker-style unit suffixes (`b`, `k`, `m`, `g`, plus `kb`/`kib`
and friends), rounded up to a whole MiB:

```bash
lns run --cpus 4 -m 2g ghcr.io/acme/builder
lns run --mem 2048 ghcr.io/acme/builder        # same thing, in MiB
```

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

Launch flags are the final override layer. Lens Sandbox starts with the mounts
from `lns.yaml`, replaces a declarative mount when an explicit `--volume` or
`--mount` targets the same guest path, and keeps mounts with other targets.

### Filesets — files shipped inside the artifact

`spec.volumes` attaches the *consumer's* files: live host binds and named
volumes on the machine where the sandbox runs. `spec.filesets` is the opposite
direction — files the *author* ships **inside** the published artifact (agent
settings, skills, prompt libraries), materialized into the guest at launch as a
read-only snapshot:

```yaml
spec:
  filesets:
    - path: ./skills              # a directory in the author's project
      mountPath: /root/.agent/skills
    - ref: ghcr.io/team/settings@sha256:…   # a pre-published FileSet artifact
      mountPath: /root/.agent/settings
```

- **`path`** names a directory in the authoring project. A local `lns run`
  snapshots it at launch — the guest sees exactly what a consumer of the
  published artifact would see (live files are `spec.volumes`' job). At
  `lns push`, each `path` directory is packed into a deterministic FileSet
  artifact, uploaded alongside the sandbox, and rewritten to a digest-pinned
  `ref` in the published config — the same publish-time pinning `spec.image`
  gets.
- **`ref`** names a pre-published FileSet artifact, always pinned by digest. A
  pulled sandbox's filesets are fetched and materialized at launch; `lns
  inspect` lists every fileset (`fileset: <ref> -> <mountPath>`) so you can
  review what a sandbox ships before running it, and the run summary
  discloses them as `Fileset:` lines.
- Each entry sets exactly one of `path`/`ref`. `mountPath` is an absolute
  guest path; duplicates — including collisions with a volume `target` — are
  rejected offline.
- A secret-shaped file (`.env`, keys, credential stores) anywhere in a `path`
  fileset refuses `validate`, `run`, and `push` outright: a fileset is baked
  into an artifact, so there is no keep/drop prompt to catch it later — real
  secrets stay outside the workload.

The trust model is pinning plus disclosure: a published sandbox whose fileset
ref is not digest-pinned is refused, and what a sandbox ships is always
visible in `lns inspect` before anything runs.

### Host bind mounts

When the source of a `-v` is an **absolute host path** rather than a name, it's a
host bind: the workload sees your live host files at the target, Docker-style.

```bash
lns run -v "$(pwd)":/work ghcr.io/acme/agent        # the agent edits your repo
lns run -v /etc/myapp:/config:ro ghcr.io/acme/app   # read-only
```

The format is `/host/path:/absolute/target[:ro]`. The source must be an absolute
path that already exists (a missing path is refused, not silently created — the one
deliberate divergence from `docker run`). Binds default to read-write; append `:ro`
for read-only. Disambiguation is by shape: a leading `/` is a host bind, anything
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

On the other side, `lns pull` fetches a published sandbox and its base image into
the local cache, and `lns run` can boot it straight from the reference:

```bash
lns pull ghcr.io/acme/reviewer:1.0.0     # pre-warm the cache
lns run  ghcr.io/acme/reviewer:1.0.0     # run it
```

`lns push` preserves `workdir` and every mount declaration in the artifact.
Consumers resolve relative binds against their own project directory, not the
publisher's. Preflight pins the resolved artifact digest; after `lns pull` has
cached that artifact and its referenced OCI content, the published sandbox can
start offline from the cached snapshot.

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

### Exec — another session inside a run

`lns exec 7 -- bash` opens a second session, like `docker exec`. The run id is
shown by `lns run -d` and `lns ps`. `-i`, `-t`, and `--detach-keys` work as they do
for `lns run`; detaching from an exec session closes only that session — the run
and any other sessions keep going. This is also how you open a debugging shell
alongside a misbehaving workload without disturbing it.

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
**cached** reference it prints the artifact's kind and definition — a plain `Image`,
or a `Sandbox`'s image, workdir, mounts, declared ports, filesets, and integrations,
flagging a permissive default policy.

### Listing resource use

`lns ps` lists running sandboxes with their CPU share and memory — the microVM is
the workload, so the numbers cover everything the run is doing.

## Cleaning up the cache

Cached sandboxes accumulate as you pull and build. Remove one with `lns rm` (a
shortcut for `lns sandbox rm`); it frees the layers no remaining cached sandbox
shares and refuses a reference that a run is still using:

```bash
lns rm ghcr.io/acme/reviewer:1.0.0
```

`lns sandbox prune` removes every cached sandbox not held by a running one,
reclaiming the disk at once. It requires `-f`/`--force` — there is no interactive
prompt, so nothing is swept until you ask for it explicitly:

```bash
lns sandbox prune --force
```

Removing a cached sandbox is always safe in the durable sense — the next `lns run`
or `lns pull` simply fetches or rebuilds it again.

## See also

- [Policy and approvals](policy.md) — control what the workload may reach.
- [Credentials](credentials.md) — give the workload placeholders, not secrets.
- [CLI reference](cli-reference.md) — the full flag list.
