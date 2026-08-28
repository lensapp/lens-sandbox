# CLI specification — `lns`

Normative reference for the `lns` command-line interface: every command, what it
takes, what it prints, what it asks, and what it returns.

> **Status: the target, not today.** This document is the agreed destination for
> the CLI surface. It describes that destination as settled — nothing below
> hedges about implementation state — and parts of it do not ship yet.
>
> For what `lns` accepts today, read the **[CLI reference](cli-reference.md)**.
>
> The product is pre-1.0. Every gap between this document and the code closes as
> a breaking, unversioned change with no compatibility shim.

For the document `lns` runs, see the
[Sandbox definition specification](sandbox-spec.md). For task-oriented guides,
start at the [documentation index](README.md).

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, and **MAY** are
used as described in RFC 2119.

---

## 1. The model

### 1.1 Artifacts and sandboxes

Two things carry a name, and every command acts on one of them.

An **artifact** is one `lns.run/v1` document, published as one OCI artifact. It
has a kind:

| Kind | What it is |
|---|---|
| `sandbox` | A complete sandbox: its base image, the egress it needs, the credentials it needs, and the files, tools, and mounts it brings. |
| `connector` | How a credential for a service is obtained and injected. |
| `mixin` | A capability layered onto a sandbox: tools, filesets, egress, credentials. |

A **sandbox** is what a sandbox artifact becomes when it runs: a live guest with
an id, a name, and a state. It outlives its workload — when the workload exits,
the sandbox stays as **stopped** until you remove it, and `lns start` runs it
again.

Each has its own namespace:

| Namespace | Acts on | Addressed by |
|---|---|---|
| [`lns artifact`](#31-lns-artifact) | The document, of any kind. | `REF` — a registry coordinate or a local path. |
| [`lns sandbox`](#32-lns-sandbox) | The running or stopped sandbox. | `RUN` — an id or a name. |

`lns sandbox run <REF>` is the one command that crosses between them: it takes an
artifact and produces a sandbox.

The rest of the surface manages neither a document nor a guest:
[`lns connector`](#33-lns-connector) decides what this machine and this project
may use; [`lns volume`](#34-lns-volume) holds data;
[`lns config`](#35-lns-config) holds your defaults; and
[`lns audit`](#36-lns-audit), [`lns service`](#37-lns-service),
[`lns login`](#38-lns-login--lns-logout), and
[`lns update`](#39-lns-update--lns-uninstall) act on this machine.

### 1.2 The kind is in the document

`lns artifact` works on all three kinds. You never tell a command which kind it is
handling — `push`, `pull`, `tag`, `validate`, `inspect`, and `rm` read that from
the document. There is no `lns connector pull` and no `lns mixin push`.

`lns artifact init` is the exception: it writes a document that does not exist
yet, so the kind is an argument — `--kind sandbox` (the default), `--kind mixin`,
or `--kind connector`.

### 1.3 Two names for one command

Common verbs are also spelled at the top level, and each is an exact alias — same
arguments, same output. Every command below names its shortcut where it has one.

`rm` and `inspect` exist in both namespaces, so their shortcuts work out which
you meant:

1. A path-shaped operand is a local document — the artifact.
2. Otherwise `lns` looks the word up in both namespaces.
3. One match runs, and the output names what it acted on.
4. Two matches refuse, and print the two namespaced commands. `lns` never guesses
   here.
5. No match is an error that names both namespaces as searched.

`prune` has no shortcut. It takes no operand, so nothing could tell them apart:
`lns artifact prune`, `lns sandbox prune`, and `lns volume prune` always name what
they sweep.

### 1.4 `lns` is a client

The `lns-service` background process owns the microVM lifecycle, the caches, the
approval window, and the audit chain. `lns` sends it one request and renders the
answer. Start it once with `lns service start`.

These commands need no service, and work offline:

| Offline | What it touches |
|---|---|
| `lns artifact init` | Writes one document. |
| `lns artifact validate` | Reads one document. |
| `lns artifact inspect <path>` | Reads one document and the mixins it names locally. |
| `lns config <verb>` | Reads and writes your config file. |
| `lns connector list` | Reads what is installed on this machine. |
| `lns --help`, `lns --version` | Nothing. |

Every other command reports the service as unreachable in one sentence that names
`lns service start`, rather than failing with a socket error.

---

## 2. Invocation

### 2.1 Shape

```text
lns [GLOBAL OPTIONS] <command> [<subcommand>] [OPTIONS] [OPERANDS] [-- WORKLOAD COMMAND...]
```

A namespace command takes exactly one subcommand, and the surface is never three
levels deep. `lns` with no command, or a namespace with no subcommand, prints help
and exits non-zero.

### 2.2 Global options

| Option | Default | Meaning |
|---|---|---|
| `--log-level <LEVEL>` | `warn` | `error`, `warn`, `info`, or `debug`. `info` adds progress lines; `debug` adds traces and the guest boot transcript. `LNS_LOG` or `RUST_LOG` override it. |
| `-V`, `--version` | | Print the version. |
| `-h`, `--help` | | Print help. On `lns run`, `-h` is the hostname, so spell out `--help` there. |

A global option is accepted anywhere before the workload command.

### 2.3 Where `lns` ends and the workload begins

`lns run` and `lns exec` take a command to run **inside** the sandbox, and that
command has flags of its own. Three rules decide the boundary, in order:

1. **`--` ends `lns`'s arguments.** Everything after it belongs to the workload,
   whatever it looks like.
2. **A flag `lns` declares stays `lns`'s**, wherever you write it.
3. **The first bare word after the reference starts the workload command.**
   Everything from there on is the workload's.

So `lns run alpine ls -la` passes `-la` to `ls`, and `lns run node --version` asks
node, not `lns`. `-it` and `-ti` expand to `-i -t`.

### 2.4 Operands

| Operand | Written as |
|---|---|
| `REF` | A registry coordinate (`ghcr.io/team/hermes:1.4.0`, `you/agent`, `name@sha256:…`) **or** a path to a local document (`.`, `./dir`, `lns.dev.yaml`). |
| `RUN` | A sandbox's numeric id (`7`), its name (`reviewer`), or a unique id prefix. |
| `PATTERN` | A destination: host, wildcard host (`*.github.com`), CIDR, or `host:port`. |
| `NAME` | A volume name, a connector id, or a registry host. |
| `KEY` | A dotted config key (`run.mem`). |

- A command takes a `REF` or a `RUN`, never either. Given the wrong kind, it says
  which it wanted and which command takes the one you typed.
- An id and a name are interchangeable wherever a `RUN` is accepted. A name is
  never all digits, so the two can never collide.
- A bare `REF` is qualified, never guessed: `--registry`, then your
  `run.registry` default, then the Lens hub (`hub.lns.run`). A fully-qualified
  reference is used as written.

---

## 3. Commands

### 3.1 `lns artifact`

Author, publish, fetch, and cache documents of any kind.

```bash
lns artifact init [--kind <sandbox|mixin|connector>] [-f <FILE>]
lns artifact validate [--kind <KIND>] [-f <FILE>]
lns artifact push <REF> [--dry-run] [--yes] [-f <FILE>]
lns artifact pull <REF> [--yes]
lns artifact tag <SOURCE> <TARGET>
lns artifact ls [--kind <KIND>] [--format <table|json>]
lns artifact inspect [REF] [--mixin <REF>]... [-f <FILE>]
lns artifact rm <REF>
lns artifact prune [-f]
```

| Verb | Shortcut | What it does |
|---|---|---|
| `init` | `lns init` | Scaffolds a document in this directory. `--kind` chooses which — `sandbox` (the default), `mixin`, or `connector`; the file is `./lns.yaml` unless `-f` names another. Refuses to overwrite. |
| `validate` | | Checks the document named by `-f`, or `./lns.yaml`, offline — schema, cross-field, and secret checks — and lists every problem it found, not just the first. That list is the answer, so it goes to stdout ([§4.1](#41-streams)), and the exit code is non-zero when the document is broken. `--kind <KIND>` also requires the document to be that kind. |
| `push` | `lns push` | Publishes the document as one OCI artifact at `<REF>`. Each `spec.filesets` `path` directory is packed into a layer of that same artifact, so the files and the declaration that mounts them share one digest; a `README.md` beside the document is packed into a `text/markdown` layer (`sandbox-spec.md` §7.2); each fuzzy `spec.tools` version is resolved and published as an exact pin. For a `spec.mixins` entry that names a local path, the document it names publishes first as its own artifact, beside `<REF>` and under the mixin's own `name`, tagged with its own digest, and the entry is pinned to that digest — the command lists those mixins and asks before it uploads anything, which `--yes` accepts without prompting. `--dry-run` does everything except the upload, stays offline, prints the digests that would publish for every artifact, and says when a declared tool means a real digest may differ. |
| `pull` | `lns pull` | Shows you what the reference resolves to, then fetches it and its base image into the local store. A sandbox that declares tools asks before running their installers in a disposable provisioning guest; `--yes` accepts that without prompting. The fetch is bound to the digest you were shown, so a tag that moves in between is refused. |
| `tag` | `lns tag` | Re-references a cached artifact under a new tag, in its own registry and repository. |
| `ls` | | Lists cached artifacts: reference, kind, digest, size, and what holds each. `--kind` filters. |
| `inspect` | `lns inspect` | Renders one artifact's resolved content, offline for a local path. `--mixin <REF>` resolves that mixin in first (repeatable), so you can preview a composition without running it. Against a local path the flag takes a local path too: the mixin it names is read from this machine and merged by the [`sandbox-spec.md` §3.3.2](./sandbox-spec.md#332-merge-rules) rules — after the document, in flag order — so the render stays offline and resolves nothing. A published reference there is refused, because an offline render has nothing to resolve it against; the message names the two ways out, a local path or a published target. A mixin the document itself declares merges the same way, when it names a local path; one the document declares by published reference is listed and not merged, and the line says so, because refusing the render over it would refuse every document written to publish. With no operand it renders `./lns.yaml`. |
| `rm` | `lns rm` | Removes one cached artifact and frees its now-unreferenced layers. Refused while a sandbox holds it — running or stopped — and the message names the holder. |
| `prune` | | Removes every cached artifact nothing holds, and — when no sandbox is live — the provisioned tool cache, which any running sandbox may still be using. Lists them and asks, unless `-f`/`--force`. |

`-f`/`--file` selects a document other than `./lns.yaml`; its directory is the
project, so it roots the document's relative binds and filesets and holds the
project's own mixin ([§3.2.3](#323-the-projects-own-mixin)).

A mixin and a connector are documents you write in an editor. Every artifact verb
here works on them exactly as it works on a sandbox, because each reads the kind
from the document itself.

### 3.2 `lns sandbox`

Create, run, watch, and remove sandboxes.

```bash
lns sandbox run [OPTIONS] [REF] [-- COMMAND...]
lns sandbox start <RUN> [-a] [-i] [--detach-keys <CHORD>]
lns sandbox stop <RUN> [-t <SECONDS>]
lns sandbox kill <RUN> [--signal <SIG>]
lns sandbox exec [OPTIONS] <RUN> [-- COMMAND...]
lns sandbox logs <RUN> [-f]
lns sandbox attach <RUN> [--detach-keys <CHORD>]
lns sandbox ls [-a] [--format <table|json>]
lns sandbox inspect <RUN> [--format <table|json>]
lns sandbox rm <RUN> [-f]
lns sandbox prune [-f]
```

#### 3.2.1 `lns run`

Shortcut for `lns sandbox run`.

`REF` is a registry coordinate or a path to a local document. Omit it to run the
`./lns.yaml` in the current directory, or select another file with `-f`. A
`COMMAND` after the reference replaces the base image's default command while
keeping its `ENTRYPOINT`; `--` is accepted but not required.

| Option | Default | Meaning |
|---|---|---|
| `-f`, `--file <FILE>` | `./lns.yaml` | Document to run instead of `./lns.yaml`. Cannot be combined with `REF`. |
| `--name <NAME>` | auto | Name the sandbox, usable in place of its id everywhere. Auto-generated (`adjective_noun`) when omitted; never all digits. |
| `--registry <HOST>` | `hub.lns.run` | Registry that qualifies a bare reference. |
| `--cpus <N>` | `1` | vCPUs. Falls back to the `run.cpus` default. |
| `-m`, `--mem <SIZE>` | `512` | RAM in MiB, or with a unit (`2g`, `512m`, `38Gi`; binary, rounded up to a whole MiB). Falls back to `run.mem`. Alias: `--memory`. |
| `--mixin <REF>` | | Merge a mixin after the ones the document declares. Repeatable, in the order written; a later one wins. Takes a reference or a path, so this is also how you run with a mixin the project does not name. A tag is pinned before the run reports it. |
| `-w`, `--workdir <DIR>` | `spec.workdir`, then image `WORKDIR` | Working directory inside the sandbox. Absolute; created if missing. |
| `-e`, `--env <KEY=VALUE>` | | Set a non-secret environment variable. Repeatable. Secrets go through the credential flow, not `-e`. |
| `--env-file <FILE>` | | Read `KEY=VALUE` lines into the workload env. Repeatable; later files and `-e` win. |
| `-v`, `--volume <SPEC>` | | Mount into the workload: `name:/path[:ro]`, `/host/path:/path[:ro]`, or `type=bind\|volume,source=…,target=…[,readonly]`. Repeatable. Alias: `--mount`. |
| `-p`, `--publish <SPEC>` | | Publish a guest port as `[host_ip:]hostport:containerport[/proto]`. Repeatable. Host bind defaults to `127.0.0.1`. Wins over a declared `spec.ports` entry for the same container port. |
| `-i`, `--interactive` | `true` | Keep stdin open. Turn off with `--interactive=false`. |
| `-t`, `--tty` | `true` | Allocate a PTY; pipe mode is selected automatically when stdin is not a terminal. Turn off with `--tty=false`. |
| `-d`, `--detach` | `false` | Return once the sandbox is started. Conflicts with `-i` and `-t`. |
| `--detach-keys <CHORD>` | `ctrl-p,ctrl-q` | Detach chord: single characters or `ctrl-X`, comma-separated. |
| `--rm` | `false` | Remove the sandbox once the workload exits, writable layer included. |
| `-u`, `--user <USER[:GROUP]>` | image `USER`, else `sandbox` | Run-as user or uid inside the sandbox. Outranks `spec.user`, which outranks the image. `HOME` and `USER` follow that user's guest passwd entry unless `env:` or `-e` sets them. |
| `--entrypoint <COMMAND>` | image `ENTRYPOINT` | Override the image entrypoint; the `COMMAND` after the reference becomes its arguments. `--entrypoint ""` clears it. |
| `-h`, `--hostname <NAME>` | | Guest hostname for this sandbox. |
| `--yes` | `false` | Accept the tool installers, `pre-start` scripts, host binds, named volumes, and filesets a pulled sandbox declares. Required for a non-interactive run that declares any of them. |
| `-q`, `--quiet` | `false` | Suppress the launch banner and `✓` lines. Warnings, errors, and the workload's output still print. |

Two things a run does without being asked to:

- **It publishes the ports the document declares.** A `spec.ports` entry is a
  statement that the sandbox serves on that port, so it is published on loopback
  — the `host` value when the entry names one, the container number otherwise.
  `-p` adds to that, and overrides an entry for the same container port.
- **It resolves this project's own mixin.** The `lns-local-mixin.yaml` beside the
  document is always present and always last in the merge, so the decisions you
  made here apply without naming a file.

Before anything boots, `lns run` prints what it resolved: the image, the mixins,
the egress, the credentials, the mounts, the tools, the ports, and the `pre-start`
scripts — each script named with the user it asks for and the document that
contributed it. That summary is the one thing you approve.

#### 3.2.2 The rest

| Verb | Shortcut | What it does |
|---|---|---|
| `start` | `lns start` | Runs a stopped sandbox again. The launch replays exactly as recorded — image, command, env, mounts, ports, resources, run-as — while the network rules and credentials re-resolve as they would for a fresh boot, and the **recorded** `pre-start` scripts run again — the ones the approved launch resolved, not a fresh resolution. Detached by default; `-a` attaches, `-i` also forwards stdin, and `--detach-keys <CHORD>` (default `ctrl-p,ctrl-q`) sets the chord that returns your terminal from an attached start. A conflict (a taken host port, a held volume, a missing bind source) aborts the start and leaves the sandbox stopped. |
| `stop` | `lns stop` | Asks the workload to exit, then escalates to `SIGKILL` after `-t <SECONDS>` (default `10`). Reports whether it had to escalate. A run still inside its `pre-start` scripts has no workload yet; `stop` ends it too. |
| `kill` | `lns kill` | Sends one signal and returns. `--signal` takes `TERM` (default), `INT`, `QUIT`, `HUP`, `WINCH`, or `KILL`, bare or `SIG`-prefixed, case-insensitive. |
| `exec` | `lns exec` | Runs another command inside a running sandbox. `-i` and `-t` are spelled as on `run` but are off unless you ask for them ([§7.3](#73-terminals)), `--detach-keys` closes only this session, and `-q` silences the status lines. |
| `logs` | `lns logs` | Prints the captured output. `-f`/`--follow` streams until the workload exits. The service keeps the most recent 2 MiB per sandbox. |
| `attach` | `lns attach` | Re-joins the live session, most useful after `run -d`. The detach chord leaves the sandbox running and returns you to your shell; no signal is sent. Stdin reaches the workload only if the sandbox was started with stdin open. |
| `ls` | `lns ps` | Lists running sandboxes with their state, CPU, and memory. `-a` includes stopped ones. |
| `inspect` | `lns inspect` | Prints one sandbox's live state and launch configuration, with its resolved mixin embedded. |
| `rm` | `lns rm` | Removes a stopped sandbox: its record and its writable layer, the name freed and the artifact released. `-f`/`--force` stops a running one first. |
| `prune` | | Removes every stopped sandbox, writable layers included. Lists them and asks, unless `-f`/`--force`. |

Interrupting `lns` at the terminal (`Ctrl-C`) stops the sandbox. The detach chord
does not: it returns your terminal, exits `0`, and leaves the workload running.

#### 3.2.3 The project's own mixin

Every project directory has one mixin nobody publishes: `lns-local-mixin.yaml`,
beside the document. It holds what you decided here — the destinations you
approved or refused, and which connectors this project uses — and every run in
that directory resolves it last, so nothing you pulled can overrule it.

- **The run writes it.** A destination no rule decides is asked about at first
  use, and your answer is appended as an `egress` entry. There is no command to
  run: answering the prompt is what records the decision.
- **You can edit it, and commit it.** It is a `kind: mixin` document, so
  `lns artifact validate` checks it and `lns artifact inspect` renders it. A
  project's agreed destinations are reviewable in a pull request instead of
  rediscovered by every developer.
- **Order is behaviour.** The guest stops at the first entry that matches a
  destination, so an entry placed after a `deny` — or after an entry that
  restricts which callers or requests it permits — does not apply. When you edit
  by hand, put the specific entry first.
- **`deny: '*'` closes the project.** Nothing unlisted gets out, and nothing
  prompts.

`lns run --mixin <PATH>` layers another mixin on top for one run, which is how you
try a rule set without writing it here.

### 3.3 `lns connector`

> **Work in progress.** This namespace is not settled. The verbs and their split
> across machine and project are still being worked out, and this section will
> change.

A connector is an artifact, so it is authored, published, and fetched with
[`lns artifact`](#31-lns-artifact). This namespace covers the two things that are
not the document: whether it is available on **this machine**, and whether it is
allowed in **this project**.

```bash
lns connector install <REF|PATH>
lns connector uninstall <ID>
lns connector list [--format <table|json>]
lns connector connect <ID> [--project <PATH>]
lns connector disconnect <ID> [--project <PATH>]
lns connector grants [--all] [--project <PATH>] [--format <table|json>]
lns connector revoke <ID> [--project <PATH>]
```

| Verb | What it does |
|---|---|
| `install` | Makes a pulled or local connector available on this machine. Installing grants nothing: a newly installed connector holds no sign-in, seeds nothing, and arms nothing. |
| `uninstall` | Removes it from this machine. |
| `list` | Lists what is installed, its sign-in methods, and whether it is connected here. |
| `connect` | Binds its value for this machine — a credential connector prompts in the approval window (use the host value, store one, or deny) and an `oauth` connector signs in — and records the connection in this project. |
| `disconnect` | Withdraws it from this project and forgets the grants remembered here. |
| `grants` | Lists the per-workload grants for this project as `workload  connector  verdict`. `--all` adds a project column and covers every project on this machine. |
| `revoke` | Forgets one connector's grants in this project, so its next use asks again. Exits `1` when there was nothing to forget. |

`--project <PATH>` acts on another project directory instead of the current one.

Three steps stay separate, and no flag collapses them: installing makes a
connector available, connecting decides this project uses it, and the workload
still meets a first-use card the first time it reaches for the value.

### 3.4 `lns volume`

Named volumes for `lns run -v`.

```bash
lns volume ls [--format <table|json>]
lns volume create <NAME>
lns volume inspect <NAME> [--format <table|json>]
lns volume rm <NAME>
lns volume prune [-f]
```

| Verb | What it does |
|---|---|
| `ls` | Lists volumes with their on-disk size, age, and the sandbox holding each. |
| `create` | Creates a volume ahead of its first attach. No-op if it exists. |
| `inspect` | Shows capacity, on-disk bytes, creation time, and holder. |
| `rm` | Removes a volume and its data. Refused while a sandbox holds it. |
| `prune` | Removes every volume no sandbox holds. Lists them and asks, unless `-f`/`--force`. |

A volume reports the space it actually occupies, not the size it was provisioned
at.

### 3.5 `lns config`

Your own defaults, applied when the matching `lns run` flag is absent.

```bash
lns config set <KEY> <VALUE>...
lns config get <KEY> [--format <table|json>]
lns config unset <KEY>
lns config list [--format <table|json>]
```

| Key | Matching flag | Value |
|---|---|---|
| `run.cpus` | `--cpus` | Number of vCPUs. |
| `run.mem` | `--mem` | RAM in MiB. |
| `run.registry` | `--registry` | Registry host for bare references. Unset means `hub.lns.run`. |

Values are validated on the way in, by the same parsers the flags use. `get` on an
unset key exits `1`, printing nothing as a table and `null` as JSON. Environment variables, volumes, and ports
are properties of a sandbox, not defaults: declare them in the document or pass
them per run.

### 3.6 `lns audit`

One chronological timeline, newest first, across every sandbox — or scoped to one.

```bash
lns audit [SANDBOX] [--connector <ID>] [--kind <KIND>] [--format <table|jsonl>]
```

`SANDBOX` is a `RUN`. `--kind` takes one of `launch`, `exit`, `restart`,
`sandbox_run`, `run_removed`, `runs_pruned`, `egress`, `env`, `volume`, `bind`,
`approval`, `connection`, `credential`, or `tool`. Filters compose.

A sandbox's own life is in the timeline as much as what it reached for: `launch`
is the workload starting, `exit` is it ending, `restart` is a stopped sandbox
running again, `sandbox_run` records the run against the artifact it ran, and
`run_removed` and `runs_pruned` are the sandbox being removed — one by name, or
swept with every other stopped one.

It is not `lns sandbox audit`, because half of what it reads is not a sandbox's:
the timeline merges each sandbox's own chain with the durable ledger of
approvals, connections, and credential decisions, which spans sandboxes and
outlives every one of them. A sandbox's chain also outlives the sandbox —
removing one does not remove what it did.

- Integrity is checked as the log is read. A chain that has been altered,
  truncated, or cannot be verified against its anchor raises an inline
  `audit integrity:` warning, and the events are still listed. There is no
  separate verify step.
- A sandbox with no events prints that and exits `0`.

### 3.7 `lns service`

```bash
lns service start | stop | status [--format <table|json>] | enable | disable
```

| Verb | What it does |
|---|---|
| `start` | Starts the background service and waits until it is ready. |
| `stop` | Stops it. |
| `status` | Reports whether it is running, with PID, uptime, and version. |
| `enable` | Registers a per-user login agent and starts the service now and at every login. |
| `disable` | Stops the service and unregisters the login agent. |

Nothing here installs a system-wide daemon. `lns start` is a sandbox; the service
is always named.

### 3.8 `lns login` / `lns logout`

Credentials for a private OCI registry, so `lns pull` and `lns run` can fetch from
it. Several registries can be logged in at once.

```bash
echo "$TOKEN" | lns login -u <USERNAME> --password-stdin [REGISTRY]
lns login --list
lns logout [REGISTRY]
```

| Form | What it does |
|---|---|
| `lns login [REGISTRY]` | Logs in to `REGISTRY`, defaulting to `run.registry`, else `hub.lns.run`. Give the secret with `--password-stdin` (recommended) or `-p`/`--password`, and the user with `-u`/`--username`. The credential is verified against the registry before it is stored, so the service must be running. |
| `lns login --list` | Lists the registries you are logged in to. It is a list like any other ([§4.2](#42-human-output), [§4.3](#43-machine-output)): a table of registry and username, `--format <table\|json>`. Never secrets. |
| `lns logout [REGISTRY]` | Removes the stored credential. |

A registry is matched by host: a fully-qualified reference uses that host's stored
login if there is one.

### 3.9 `lns update` / `lns uninstall`

```bash
lns update [--force] [--dry-run]
lns uninstall [--purge] [-y]
```

| Option | What it does |
|---|---|
| `update --force` | Reinstalls even when the running version already matches. |
| `update --dry-run` | Prints the anonymous update-check payload that would be sent — install id, version, OS, architecture — and contacts nothing. |
| `uninstall --purge` | Also deletes everything under `~/.lns/`: cached artifacts and layers, volumes, the audit trail, config, and stored credentials. Without it, only the program is removed. |
| `uninstall -y`, `--yes` | Skips the confirmation. |

`lns uninstall` stops running sandboxes, stops the service, removes the login
agent, deletes the binaries, and reports what it kept. Neither command announces
an available update while it runs.

---

## 4. Output

### 4.1 Streams

**stdout carries the answer** — the table, the JSON, the value you asked for, the
workload's own output. **stderr carries everything else** — progress, status
lines, warnings, errors, prompts, and log output at every level.

So `lns ps --format json | jq` works, `lns logs 7 > file` captures the workload
and nothing else, and redirecting stdout never hides a warning or a prompt.
`lns artifact validate` follows the same rule rather than the shape of what it
prints: the problem report is the answer you asked for, so it is stdout, and the
non-zero exit is what says the document is broken.

### 4.2 Human output

- A list prints a table with an uppercase header row.
- An empty list prints one sentence saying so, not an empty table.
- Progress and completion lines are prefixed `✓` and go to stderr. `-q` suppresses
  them.
- Colour is emphasis only, never the sole carrier of meaning, and is off when
  `NO_COLOR` is set or the stream is not a terminal.

Human output carries **no stability promise**.

### 4.3 Machine output

Every command that answers with data takes one flag:

```text
--format <table|json>      # a list, or one object
--format <table|jsonl>     # a stream of events, one JSON object per line
```

`table` is the default. `jsonl` is what a timeline offers, because a reader has to
act on each event as it arrives; `lns audit` is the one command that takes it. No command offers both spellings of the same thing, and
there is no separate `--json` switch.

### 4.4 The JSON contract

- A list is a **bare array of objects**, pretty-printed. A command that answers
  about one thing emits a **single object**.
- Keys are **camelCase** and **always present**. A key with no value is `null`,
  never omitted, so `jq .inUseBy` needs no guard.
- Numbers are **raw**: `sizeBytes: 92274688`, not `"88.0 MiB"`. Timestamps pass
  through as the service reports them.
- The **exit code is the same** in both formats: `lns config get` on an unset key
  exits `1` either way, printing nothing as a table and `null` as JSON.
- An empty list is `[]`. A single thing that does not exist is `null`.
- JSON may carry **more** fields than the table has room for. The table is a
  summary; the JSON is the record.

---

## 5. Exit codes

| Code | Meaning |
|---|---|
| `0` | It did what you asked. |
| `1` | It did not, and `lns` is telling you why. Includes a refusal, a validation failure, and a "not found" answer such as `config get` on an unset key. |
| `2` | The invocation could not be parsed: unknown command, missing operand, unparsable value. |
| `125` | `run` or `exec` failed before the workload started — service unreachable, reference unresolved, mount invalid, declared tool refused, `pre-start` script failed. |
| `126` | The workload command was found but could not be executed. |
| `127` | The workload command was not found in the sandbox. |
| *the workload's* | Once the workload has started, `run` and `exec` return its exit status. |

The three high codes exist so a failure of `lns` is never mistaken for the
workload exiting with the same number. Detaching with the chord exits `0`.

---

## 6. Errors

An error says what `lns` was doing, what stopped it, and what to do next — in a
sentence.

A **refusal** is different from a failure: `lns` could have done it and declined,
because doing it would widen access, lose data, or hide a decision. A refusal
names what refused and leaves nothing half-written. `lns artifact rm` on an
artifact a stopped sandbox still holds is the pattern for all of them: it names
the holder, and removes nothing.

A command that writes a file leaves it either untouched or completely replaced —
never half-written. Warnings never change the exit code.

---

## 7. What `lns` asks you

### 7.1 The four questions

| Question | Raised by |
|---|---|
| Run this sandbox's declared tool installers? | `pull`, `run` |
| Let these host files in, including the secret-shaped ones? | `run` with a host bind |
| Delete this data? | `prune`, `uninstall` |
| Use this credential for this workload? | The approval window, on first use |

The first three are asked at your terminal. The fourth is not: a credential
decision is made in the approval window the background service owns, because it
has to be answerable while a sandbox is already running.

### 7.2 Answering

- A prompt is written to stderr and read from your terminal. It never consumes
  stdin, so piping data into `lns run` can never be mistaken for a "yes".
- `--yes` accepts what a document declares. `-f`/`--force` accepts a `prune`.
  `-y` accepts an uninstall.
- A verb that names its target does not ask: you already named it. `rm` removes
  what you pointed at, and `-f` on `lns sandbox rm` means "stop it first", not
  "delete without asking".
- A flag answers only the question it names. No flag answers the credential card:
  that value is bound per machine and granted per workload.
- With no terminal to ask at, a command that would have asked **refuses** and
  names the flag that would have answered it. It never assumes.

### 7.3 Terminals

On `lns run`, `-i` and `-t` are on by default, and pipe mode is selected
automatically when stdin is not a terminal — an interactive session needs no
flags, a scripted one needs no ceremony. `-d` conflicts with both.

On `lns exec` both are off by default. A run is the session you came for; an
exec is usually one command whose output you are reading or piping, so it asks
for a terminal only when you say `-it`.

---

## 8. Precedence

Every value a sandbox runs with is decided by this chain. Earlier wins; later
fills the gap.

1. **A command-line flag.**
2. **A resolved mixin**, with this project's own mixin last of all.
3. **The document's `spec`.**
4. **The base image** — `USER`, `WORKDIR`, `ENTRYPOINT`, `CMD`.
5. **Your `lns config` default**, for the keys that have one.
6. **The built-in default.**

Your config sits **below** the document on purpose: a sandbox that declares its
own resources is not silently resized by whoever cloned it. It sits above the
built-in default, which is what it is for.

An environment variable is not a configuration layer — the variables in
[§9](#9-environment-and-files) redirect or silence `lns`, and none of them sets a
value a flag could set.

Within one layer, later wins: `--env-file` then `-e`, and `-p` over a declared
port for the same container port.

---

## 9. Environment and files

| Variable | Effect |
|---|---|
| `LNS_LOG` / `RUST_LOG` | Override the log threshold. |
| `NO_COLOR` | Disable colour. |
| `LNS_HOME` | The directory below, instead of `~/.lns`. |
| `LNS_SOCKET_PATH` | The service socket to talk to. |
| `LNS_NO_UPDATE_CHECK` | Suppress the update-and-security check and its announcement. |
| `LNS_SERVICE_BIN` | The `lns-service` binary `lns service start` launches. |

Everything `lns` keeps for you lives in one directory, `~/.lns/`:

| Path | Holds |
|---|---|
| `~/.lns/config.yaml` | Your `lns config` defaults. |
| `~/.lns/connectors.yaml` | The connectors installed on this machine. |
| `~/.lns/workload-grants.json` | Which workload was granted which connector, per project. |
| `~/.lns/registry-auth.json` | Registry logins, mode `0600`. |
| `~/.lns/` (the rest) | Cached artifacts and layers, named volumes, the audit trail, and the kernel. |

One directory, one thing to back up, one thing `lns uninstall --purge` removes.

The project keeps two files, both in the directory you work in:

| Path | Holds |
|---|---|
| `./lns.yaml` | The sandbox document. |
| `./lns-local-mixin.yaml` | What you decided here: the egress rules you approved, and which connectors this project uses. Committable. |

Secrets are never written to the project. A credential value is bound per machine,
and what a project records is the decision, not the value.

---

## 10. Stability

- **Command names, operands, and exit codes are the stable part.** Scripts and
  habits are built on them, so they change only for a stated reason, and the old
  spelling is removed rather than quietly kept working.
- **JSON output is experimental until 1.0.** Field names and shapes may change in
  a minor release; pin your `lns` version in scripts that read them.
- **Human output carries no promise at all.**

The product is pre-1.0 and carries no compatibility shims. A rename is a rename.

---

## 11. See also

- [CLI reference](cli-reference.md) — every command and flag as it ships today.
- [Sandbox definition specification](sandbox-spec.md) — the document this surface
  runs.
- [Running workloads](running-workloads.md) — the guide to `run`, `ps`, `exec`,
  and `stop`.
- [Policy and approvals](policy.md) — the destination grammar, and how the
  project's own mixin records what you approve.
- [Connectors](connectors.md) and [Credentials](credentials.md) — the model behind
  `lns connector`.
- [Audit](audit.md) — the chain `lns audit` reads.
- [The background service](service.md) — what `lns` is a client of.
