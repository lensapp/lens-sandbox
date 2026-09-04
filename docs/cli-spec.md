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
| `mixin` | A capability layered onto a sandbox: tools, filesets, egress, credentials. |
| `connector` | A mixin you install on this machine, offered when a run reaches a service it covers. Nothing it carries applies until you grant it to a run. |

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
[`lns connector`](#33-lns-connector) decides what this machine offers;
[`lns volume`](#34-lns-volume) holds data;
[`lns config`](#35-lns-config) holds your defaults; and
[`lns audit`](#36-lns-audit), [`lns service`](#37-lns-service),
[`lns login`](#38-lns-login--lns-logout), and
[`lns update`](#39-lns-update--lns-uninstall) act on this machine.

### 1.2 The kind is in the document

`lns artifact` works on all three kinds. You never tell a command which kind it is
handling — `push`, `pull`, `tag`, `validate`, `inspect`, and `rm` read that from
the document. There is no `lns connector pull` and no `lns mixin push`.

Two commands write a document that does not exist yet, so each takes the kind as
an argument. `lns artifact init` takes `--kind sandbox` (the default),
`--kind mixin`, or `--kind connector`. `lns sandbox save` takes `--kind sandbox`
(the default) or `--kind mixin`.

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
| `RUN` | A sandbox's id (`1a2b3c4d0000`, shown to 12 characters), its name (`reviewer`), or a unique id prefix. |
| `PATTERN` | A destination: host, wildcard host (`*.github.com`), CIDR, or `host:port`. |
| `NAME` | A volume name, a connector id, or a registry host. |
| `CONNECTION` | A connector connection's label, unique per connector. |
| `KEY` | A dotted config key (`run.mem`). |

- A command takes a `REF` or a `RUN`, never either. Given the wrong kind, it says
  which it wanted and which command takes the one you typed.
- An id and a name are interchangeable wherever a `RUN` is accepted. A name is
  never all lowercase hex, so the two can never collide — an id and any prefix of
  one are lowercase hex, and a name is refused if it is.
- `RUN` names a run that exists. `lns connector grant --run` and `lns connector
  forget --run` are the one exception: both also take a name no run holds yet.
  `grant` reserves the decision for the run the user next creates with that name,
  and `forget` clears a reservation waiting for it
  ([sandbox-spec §3.2.4](sandbox-spec.md#324-installing-connecting-and-applying)).
  A name is never all lowercase hex, so an id or id prefix that resolves to
  nothing is an error there, never a reservation.
- A bare `REF` is qualified, never guessed: `--registry`, then your
  `run.registry` default, then the LNS hub (`hub.lns.run`). A fully-qualified
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

`-f`/`--file` selects a document other than `./lns.yaml`. The **document's own**
directory is the project, so it roots that document's relative binds and
filesets. The working directory and the document's directory decide nothing else
([§3.2.3](#323-the-runs-decisions-and-saving-them)).

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
lns sandbox save <RUN> -f <FILE> [--kind <sandbox|mixin>]
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
| `--name <NAME>` | auto | Name the sandbox, usable in place of its id everywhere. Auto-generated (`adjective_noun`) when omitted; never all lowercase hex. |
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
| `--rm` | `false` | Remove the sandbox once the workload exits, writable layer included. What the run decided goes with it ([§3.2.3](#323-the-runs-decisions-and-saving-them)). |
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
- **It gives the run its own decisions file.** Every run has one, in the run's
  own directory, always last in the merge. What you answer at a prompt is written
  there, so it applies for the rest of that run and never leaks into the next
  one.

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
| `ls` | `lns ps` | Lists running sandboxes with their state, when each was created, when it last booted, CPU, and memory. A sandbox is created once and boots as often as you start it, so the two times differ on anything restarted. `-a` includes stopped ones. |
| `inspect` | `lns inspect` | Prints one sandbox's live state and launch configuration, with its resolved mixin embedded. It reports the same two times `ls` does, under the same names. |
| `save` | | Writes one sandbox out as a document you keep ([§3.2.3](#323-the-runs-decisions-and-saving-them)). `-f`/`--file` names the file and is required; `--kind mixin` writes what the run decided instead of the run as it resolved. Works on a running or a stopped sandbox. |
| `rm` | `lns rm` | Removes a stopped sandbox: its record and its writable layer, the name freed and the artifact released. What it granted, declined, and decided goes with it, so save anything worth keeping first ([§3.2.3](#323-the-runs-decisions-and-saving-them)). `-f`/`--force` stops a running one first. |
| `prune` | | Removes every stopped sandbox, writable layers included, and what each granted, declined, and decided. Lists them and asks, unless `-f`/`--force`. |

Interrupting `lns` at the terminal (`Ctrl-C`) stops the sandbox. The detach chord
does not: it returns your terminal, exits `0`, and leaves the workload running.

#### 3.2.3 The run's decisions, and saving them

Every run has one mixin nobody publishes: `decisions.yaml`, in the run's own
directory. It holds what that run decided — the destinations you approved or
refused — and the run resolves it last, so neither what you pulled nor what you
connected can overrule it.

- **The run writes it.** A destination no rule decides is asked about at first
  use, and your answer is appended as an `egress` entry. There is no command to
  run: answering the prompt is what records the decision.
- **It belongs to that run.** A second `lns run` is a second run and starts with
  an empty file, so it asks for itself. `lns start` and `lns exec` rejoin the
  same run and keep what it decided.
- **It goes when the run goes.** `lns rm` and `lns prune` take it with the run.
- **Order is behaviour.** The guest stops at the first entry that matches a
  destination, so an entry placed after a `deny` — or after an entry that
  restricts which callers or requests it permits — does not apply.
- **`deny: '*'` closes the run.** Nothing unlisted gets out, and nothing prompts.

`lns sandbox save` is how a decision outlives the run that made it.

```bash
lns sandbox save <RUN> -f <FILE> [--kind <sandbox|mixin>]
```

| Option | Default | Meaning |
|---|---|---|
| `-f`, `--file <FILE>` | — | Where to write. **Required**: `lns` never picks a path in your directory for you. Refuses to overwrite an existing file. |
| `--kind <KIND>` | `sandbox` | What to write. `sandbox` writes what the documents resolved to: image, command, `env`, `workdir`, `user`, mounts, ports, resources, tools, filesets, credentials, and the `egress` every document source decided, this run's own decisions included. What a connector granted stays out — a teammate is asked on their own machine ([`sandbox-spec.md` §8.6](sandbox-spec.md#86-where-a-connector-grant-goes)). Every path it carries is written absolute, so the document means the same thing from any directory. `mixin` writes what the run decided and nothing else. |

The document's `name` is the stem of the file you name, in both kinds. A stem
that is not a legal document name refuses the save and says which characters a
name takes, rather than writing a document `lns artifact validate` would reject.

What it writes is an ordinary document from that point on: `lns artifact validate`
checks it, `lns artifact inspect` renders it, and `lns artifact push` publishes
it. A saved mixin is committable, and the `sandbox` that needs it names it in
`spec.mixins` — which is how an agreed rule set reaches a teammate. Nothing
applies because a file sits in a directory.

`lns run --mixin <PATH>` layers a mixin on top for one run, which is how you try a
rule set before you save one.

### 3.3 `lns connector`

A connector is an artifact, so it is authored, published, and fetched with
[`lns artifact`](#31-lns-artifact). This namespace covers the one thing that is
not the document: whether it is available on **this machine**.

```bash
lns connector install <REF|PATH>
lns connector uninstall <ID>
lns connector list [--format <table|json>]
lns connector connect <ID> [--method <NAME>] [--as <CONNECTION>]
lns connector disconnect <ID> [--connection <CONNECTION>]
lns connector grant <ID> --run <RUN> [--method <NAME>] [--connection <CONNECTION>] [--yes]
lns connector forget <ID> --run <RUN>
```

| Verb | What it does |
|---|---|
| `install` | Makes a pulled or local connector available on this machine. Installing grants nothing: no destination opens, no file is written, and no real value is supplied. It does make the connector's destinations ask, in every run that has neither granted nor declined it, and lend its `placeholder` to a variable a sandbox itself declared and left to it. Refused when a method declares a block a connector may not carry, when the document's `serves` overlaps an installed connector's, or when it claims a variable an installed connector already claims — an `envVar` or a plain `env` key. |
| `uninstall` | Removes it from this machine, with every connection it held. A run that already granted a method keeps that decision — uninstalling stops the offer, it does not retract a grant — and the command says so. |
| `list` | Lists what is installed: what each connector serves, its methods — marking those that need no connect — and the connections this machine holds for it. |
| `connect` | Connects this machine, without waiting for a run to ask. Picks a method — `--method` names one, otherwise you choose — runs whatever authentication that method declares, and stores the result as a **connection**. A method with no `auth` has nothing to connect and is refused here — grant it instead. `--as` names it; otherwise the mechanism suggests one and you confirm. A machine may hold several connections of one connector, including several of one method: two accounts, or two sign-ins at different scopes. Connecting is not granting: a run still decides which connection to use. |
| `disconnect` | Drops one connection, or every connection of a connector when `--connection` is absent. Exits `1` when the connector holds none — a connector whose methods all lack `auth` never holds one. The connector stays installed, runs that granted a dropped connection keep their grants, and you are asked to connect again the next time a request needs a value. |
| `grant` | Grants one run a method before it asks for it. `--run` names the run and is required — there is no working directory to fall back on. Prints what the card would show — the destinations the method opens, the files it writes, the variables it sets, and the authority of each connection of the method it grants — and asks. `--yes` answers that disclosure on the command line instead of at the prompt, so one command per run gives a dispatcher its grant with no terminal and no card; the disclosure is still printed, and `--yes` still chooses nothing. `--method` names the method; omitted, you choose when more than one is offerable. `--connection` names the connection behind it, and a method with no `auth` takes none; where one authenticates and this machine holds no connection, `grant` says to `connect` first rather than starting an authentication of its own. A run holds one grant per connector, so this replaces any prior one, and what it prints names the method it displaces. Where `--run` names no run, what it prints says so, and the answer reserves the decision for the run the user next creates with that name. Exits `1` when the run already granted that method and connection, or when a reservation for that name already names them. |
| `forget` | Clears one run's decision about one connector, granted or declined, so the next start asks again. It also clears a reservation waiting for a name. The inverse of `grant`, and `--run` is required for the same reason. Exits `1` when there was nothing to forget. |

Four verbs bound the decision, across two scopes. On the machine: `install`
makes a connector offerable, and `connect` signs in and stores a connection. On a
run: `grant` lets it use one method, and `forget` takes that back. The working
directory is not a scope here — it roots the paths a reference resolves against,
and nothing about consent.

**`--as` naming a connection that already exists re-authenticates that one in
place.** Where the authentication comes back with different authority, the grants
naming that connection are invalidated and asked again
([sandbox-spec §3.2.4](sandbox-spec.md#324-installing-connecting-and-applying)).

**`grant` is the card in a terminal, not a shortcut past it.** It discloses
exactly what the card discloses and then asks, so consent is never given to a
payload nobody saw ([§7.2](#72-answering)). What it moves is *when* you decide,
not *whether* you were told.

**A person consents in one of three ways: at the card, at the terminal prompt,
or with `--yes` on `grant`.** The flag is not a script consenting on a person's
behalf: it runs on the host as that person, who already holds the credential and
can spend it directly, and the boundary's job — keeping the value out of the
guest — is the same however the grant was answered. The disclosure prints in
every one of the three, because consent by flag is still consent to something
the person could read. With neither a terminal nor `--yes`, `grant` refuses like
any other prompt ([§7.2](#72-answering)) and names both ways out. The card
itself stays unanswerable by any flag, and the answer's source — `card`,
`terminal` or `flag` — is recorded in the audit chain.

**The card is the normal path.** It offers every method the connector declares,
and for one that authenticates, every connection this machine holds plus the option
of a new one — so choosing there is what most people ever do. `connect` and
`grant` are for deciding ahead of time: signing in on a new machine, adding a
second connection, or pointing a fresh clone at the method and connection you know it
needs. Either way the sequence is the same: choose a method, and a connection where
it authenticates; satisfy the authentication where there is one; and only then do
its egress, its files, and its injections apply.

**The request that raised the card waits while you do it.** How long it waits is
not something `lns` controls — the workload set that deadline. A slow sign-in can
outlast it, and then that one request fails. It does not cancel the connect: you
finish, the connection applies, and the next request works.

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
| `ls` | Lists volumes with their on-disk size, age, and every sandbox that holds each. |
| `create` | Creates a volume ahead of its first attach. No-op if it exists. |
| `inspect` | Shows capacity, on-disk bytes, creation time, and every sandbox that holds it. |
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
`approval`, `connector`, or `tool`. Filters compose.
`connector` covers granting, declining, and forgetting one. `--connector <ID>`
keeps only what was decided about that one.

**Arming a credential is not an event of its own.** A grant records that the
credential will be armed and the connector digest it bound to, and every
destination it opens is recorded as an approval. The one thing a per-boot line
would add is a fingerprint of the value behind it — a live secret's hash in a
ledger that holds none. The timeline records the decision, not each application
of it.

**Connecting is not a connector decision the timeline holds.** Every line the
timeline carries answers for one run — but a connection is
the machine's, held by no run and offered to every one of them
([sandbox-spec §7.1](sandbox-spec.md#71-connectors)), so no run's
timeline could account for it. `disconnect` is the same.

A sandbox's own life is in the timeline as much as what it reached for: `launch`
is the workload starting, `exit` is it ending, `restart` is a stopped sandbox
running again, `sandbox_run` records the run against the artifact it ran, and
`run_removed` and `runs_pruned` are the sandbox being removed — one by name, or
swept with every other stopped one.

It is not `lns sandbox audit`, because half of what it reads is not a sandbox's:
the timeline merges each sandbox's own chain with the durable ledger of approvals
and connector decisions, which spans sandboxes and outlives every one of them. A
sandbox's chain also outlives the sandbox — removing one does not remove what it
did.

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
  never omitted, so `jq .policy` needs no guard.
- Numbers are **raw**: `sizeBytes: 92274688`, not `"88.0 MiB"`. Timestamps pass
  through as the service reports them.
- The **exit code is the same** in both formats: `lns config get` on an unset key
  exits `1` either way, printing nothing as a table and `null` as JSON.
- An empty list is `[]`, at the top level and as a key's value. A single thing
  that does not exist is `null`.
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
| Bind a value for this credential? | The approval window, when a run needs one |
| Connect this connector, and use this method here? | The approval window, when a run reaches a service it serves — or your terminal, via `lns connector connect` and `lns connector grant` |

The first three are asked at your terminal. A credential or connector decision is
asked in the approval window the background service owns, because it has to be
answerable while a sandbox is already running. The connector question is the one
you may also answer early, at your terminal, with the same disclosure.

### 7.2 Answering

- A prompt is written to stderr and read from your terminal. It never consumes
  stdin, so piping data into `lns run` can never be mistaken for a "yes".
- `--yes` accepts what a document declares. `-f`/`--force` accepts a `prune`.
  `-y` accepts an uninstall. `--yes` on `lns connector grant` accepts the
  disclosure that command prints.
- A verb that names its target does not ask: you already named it. `rm` removes
  what you pointed at, and `-f` on `lns sandbox rm` means "stop it first", not
  "delete without asking".
- A flag answers only the question it names. No flag answers the connector card:
  connecting is done per machine, and granting is decided per run.
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
2. **A resolved mixin**, with the run's own decisions last of all.
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
| `~/.lns/connectors/` | The connectors installed on this machine, each stored verbatim at the digest it came from. |
| `~/.lns/connector-grants.json` | Which connector method a run granted, the connection behind it where it authenticates, the authority it consented to, which connectors it declined, and the grants reserved for a name no run holds yet. |
| `~/.lns/connector-values.json` | The connections this machine holds — each one's authority and the values its `auth` returned, mode `0600`. |
| `~/.lns/registry-auth.json` | Registry logins, mode `0600`. |
| `~/.lns/` (the rest) | Cached artifacts and layers, named volumes, the audit trail, and the kernel. |

One directory, one thing to back up, one thing `lns uninstall --purge` removes.

A run keeps its own decisions with the run, in `~/.lns/`, and not in your project:

| Path | Holds |
|---|---|
| `~/.lns/runs/<RUN>/decisions.yaml` | What that run decided: the egress rules you approved at its prompts. It goes when the run does; `lns sandbox save --kind mixin` writes it somewhere you keep. |

**`lns` writes no file you did not point it at.** Two commands write into your
project: `lns artifact init`, to `./lns.yaml` or the `-f` you give it, and
`lns sandbox save`, to the `-f` it requires. No other command creates a file
there. The directory is read for the document you point at, and for
nothing else.

Secrets are never written to the project. A credential value is bound per machine,
and what a run records is the decision, not the value.

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
- [Policy and approvals](policy.md) — the destination grammar, and how a run's
  decisions record what you approve.
- [Audit](audit.md) — the chain `lns audit` reads.
- [The background service](service.md) — what `lns` is a client of.
