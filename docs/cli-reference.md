# CLI reference

Every `lns` command, subcommand, and flag. Run `lns <command> --help` for the same
information at the terminal.

This page describes what `lns` does today. For the normative rules the surface is
built on — the invocation grammar, the output and exit-code contracts, and what
the CLI asks before it acts — see the
[CLI specification](cli-spec.md).

## Global options

| Option              | Default | Meaning                                                                              |
| ------------------- | ------- | ------------------------------------------------------------------------------------ |
| `--log-level <LVL>` | `warn`  | Log threshold. `warn` shows warnings/errors; `info` adds progress lines; `debug` adds traces and the guest boot transcript. Override with `LNS_LOG` or `RUST_LOG`. |
| `-V`, `--version`   |         | Print the version.                                                                   |
| `-h`, `--help`      |         | Print help. (`lns run` uses `-h` for `--hostname`; spell out `--help` there.)        |

Log levels: `error`, `warn`, `info`, `debug`.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| `0` | It did what you asked. |
| `1` | It did not, and `lns` is telling you why — a refusal, a validation failure, or a "not found" answer such as `lns config get` on an unset key. |
| `2` | The invocation could not be parsed: unknown command, missing operand, unparsable value. |
| `125` | `run` or `exec` failed before the workload started — service unreachable, reference unresolved, mount invalid, declared tool refused, `pre-start` script failed. |
| `126` | The workload command was found in the sandbox but could not be executed. |
| `127` | The workload command was not found in the sandbox. |
| *the workload's* | Once the workload has started, `run` and `exec` return its exit status. |

The three high codes exist so a failure of `lns` is never mistaken for the workload
exiting with the same number. Detaching with the chord exits `0`.

## Environment and files

| Variable | Effect |
| -------- | ------ |
| `LNS_LOG` / `RUST_LOG` | Override the log threshold. |
| `NO_COLOR` | Disable colour. |
| `LNS_HOME` | The directory below, instead of `~/.lns`. |
| `LNS_SOCKET_PATH` | The service socket to talk to. |
| `LNS_NO_UPDATE_CHECK` | Suppress the update-and-security check and its announcement. |
| `LNS_SERVICE_BIN` | The `lns-service` binary `lns service start` launches. |

Everything `lns` keeps for you lives in one directory, `~/.lns/`:

| Path | Holds |
| ---- | ----- |
| `~/.lns/config.yaml` | Your `lns config` defaults. |
| `~/.lns/connectors.yaml` | The connectors declared on this machine. |
| `~/.lns/credentials.json` | The per-machine credential values you bound. |
| `~/.lns/workload-grants.json` | Which workload was granted which connector, per project. |
| `~/.lns/registry-auth.json` | Registry logins, mode `0600`. |
| `~/.lns/audit/` | Each sandbox's own audit chain, which outlives the sandbox that wrote it. |
| `~/.lns/runs/` | A live or stopped sandbox's writable layer and scratch, removed with the sandbox. |
| `~/.lns/` (the rest) | Cached artifacts and layers, named volumes, the connection ledger, and the kernel. |

One directory, one thing to back up, one thing `lns uninstall --purge` removes.

The project keeps two files, both in the directory you work in: `./lns.yaml`, the
sandbox document, and `./lns-local-mixin.yaml`, what you decided here. Secrets are
never written to the project.

## Machine-readable output

The list and status verbs take `--format <table|json>`, so a script doesn't have to
parse the human table:

`lns ps`, `lns sandbox ls`, `lns sandbox inspect <RUN>`, `lns artifact ls`,
`lns volume ls`, `lns volume inspect`, `lns connector list`, `lns connector grants`,
`lns login --list`, `lns config list`, `lns config get`, `lns service status`.

`table` is the default everywhere, including the two `inspect` verbs — the table is
a summary a reader scans, the JSON is the record. `lns artifact inspect` renders a
document as its author wrote it and takes no `--format`. The `lns inspect` shortcut
takes it, passes it through when the target settles as a sandbox, and refuses it —
saying why — when the target is a document or a cached artifact.

`lns audit` takes `--format <table|jsonl>` instead — a timeline is an event stream, so
its machine-readable form is one JSON event per line. The older `--json` spelling still
works as an alias for `--format jsonl`.

What the JSON gives you:

- **A bare array of objects** for the list verbs, pretty-printed. `lns service status`,
  `lns sandbox inspect`, `lns volume inspect`, and `lns config get` emit a single
  object instead.
- **camelCase keys**, always present — a key with no value is `null`, never omitted, so
  `jq .inUseBy` needs no guard.
- **Raw numbers**, so nothing has to be un-humanized: `sizeBytes: 92274688`, not
  `"88.0 MiB"`. Timestamps pass through as the service reports them.
- **The same exit code as the table.** `--format` changes the shape and nothing else:
  `lns config get` on an unset key still exits 1, emitting `null`.
- Some verbs report more in JSON than the table has room for. `lns sandbox ls` is the
  clearest case: the table abbreviates the digest and formats the size, and the JSON
  adds the whole digest, the raw byte count, the layer count, and the pull time.
- The empty-list sentences (`No rules in …`) become `[]`. Warnings go to stderr in both
  formats, so stdout stays parseable either way.

> **The JSON shape is experimental until v1.0.** Field names and shapes may change in a
> minor release, so pin your `lns` version in scripts that depend on them. Table output
> is for humans and carries no stability promise at all.

## Two namespaces

Two things carry a name, and every command acts on one of them.

- An **artifact** is one `lns.run/v1` document, published as one OCI artifact.
  [`lns artifact`](#lns-artifact) authors, publishes, fetches and caches it,
  addressed by a `REF` — a registry coordinate or a path to a local document.
- A **sandbox** is what a sandbox artifact becomes when it runs.
  [`lns sandbox`](#lns-sandbox) creates, watches and removes it, addressed by a
  `RUN` — an id or a name.

`lns sandbox run <REF>` is the one command that crosses between them.

Common verbs are also spelled at the top level, and each is an exact alias — same
arguments, same output:

| Top-level shortcut          | Expands to                            |
| --------------------------- | ------------------------------------- |
| `lns run [REF]`             | `lns sandbox run [REF]`               |
| `lns exec <RUN>`            | `lns sandbox exec <RUN>`              |
| `lns start <RUN>`           | `lns sandbox start <RUN>`             |
| `lns ps`                    | `lns sandbox ls`                      |
| `lns kill <RUN>`            | `lns sandbox kill <RUN>`              |
| `lns stop <RUN>`            | `lns sandbox stop <RUN>`              |
| `lns logs <RUN>`            | `lns sandbox logs <RUN>`              |
| `lns attach <RUN>`          | `lns sandbox attach <RUN>`            |
| `lns init`                  | `lns artifact init`                   |
| `lns push <REF>`            | `lns artifact push <REF>`             |
| `lns pull <REF>`            | `lns artifact pull <REF>`             |
| `lns tag <SOURCE> <TARGET>` | `lns artifact tag <SOURCE> <TARGET>`  |

`rm` and `inspect` exist in both namespaces, so their shortcuts work out which you
meant:

1. A path-shaped operand is a local document — the artifact. The service is never
   asked.
2. Otherwise `lns` looks the word up in both namespaces.
3. One match runs.
4. Two matches refuse, and print the two namespaced commands. `lns` never guesses
   here.
5. No match is an error that names both namespaces as searched.

`prune` has no shortcut: it takes no operand, so nothing could tell them apart.
`lns artifact prune`, `lns sandbox prune` and `lns volume prune` always name what
they sweep.

`validate` and `ls` have no shortcut either.

## `lns run`

Run a sandbox in a microVM.

```bash
lns run [OPTIONS] [REF] [-- COMMAND...]
```

`REF` is a **registry coordinate** (`ghcr.io/team/hermes:1.4.0`) or a **path to a
local definition** — `.`, `lns.yaml`, `./lns.yaml`, a relative/absolute path to
a directory holding one, or a path-shaped `.yaml`/`.yml` file naming the
definition itself (`./lns.dev.yaml`); omit it to run the `./lns.yaml` in the
current directory, or select another file with `-f`/`--file` (exclusive with
`REF`). A path-named definition's directory is the project: it roots the relative binds
and filesets, compose-style, and holds the `lns-local-mixin.yaml` the run
resolves. A
`COMMAND` after the reference overrides the sandbox base image's default command
(`lns run ghcr.io/acme/agent:1 echo hi`) while keeping its `ENTRYPOINT`; an explicit `--`
separator is still accepted. A command with no `REF` (`lns run -- echo hi`) runs
the `./lns.yaml` definition with its command overridden.

| Option                       | Default          | Meaning                                                                 |
| ---------------------------- | ---------------- | ----------------------------------------------------------------------- |
| `--cpus <N>`                 | `1`              | Number of vCPUs (at least 1). Without it the document's `spec.resources` decides, then the `run.cpus` config default, then `1`. |
| `-m`, `--mem`, `--memory <SIZE>` | `512`        | RAM in MiB, or with a unit suffix (`-m 2g`, `-m 512m`, `-m 38Gi` — the same sizes `spec.resources.memory` accepts, all binary, rounded up to a whole MiB). Without it the document's `spec.resources` decides, then the `run.mem` config default, then `512`. |
| `-f`, `--file <FILE>`        | `./lns.yaml`     | Definition file to run instead of `./lns.yaml` (e.g. `lns.dev.yaml`); its directory is the project, so it roots the definition's relative binds and filesets and holds the decisions file. Cannot be combined with `REF`. |
| `--name <NAME>`              | auto             | Name the run, addressable by every `lns sandbox` verb in place of its id. Omitted, a run of a document is named after it plus one word (`some-sandbox-falcon`) and any other run gets two (`amber-falcon`); must not be all digits. |
| `--registry <HOST>`          | `hub.lns.run`    | Registry to qualify a bare published-sandbox reference (e.g. `ghcr.io`); falls back to the `run.registry` config default, else the Lens hub. A fully-qualified reference is used as-is. |
| `--mixin <REF>`              |                  | Merge a mixin into this run, after the ones the document declares (repeatable, in flag order — a later one wins). Takes a reference or a directory. A bare reference is qualified by `--registry`, else `run.registry`, else the Lens hub; a directory is a local document and stays a path. A tag is allowed and is pinned before the run reports it; the summary shows `tag → digest`, and a directory shows its absolute path. |
| `-w`, `--workdir <DIR>`      | `spec.workdir`, then image `WORKDIR` | Working directory inside the sandbox (absolute path; created if missing). |
| `-e`, `--env <KEY=VALUE>`    |                  | Set a non-secret environment variable (repeatable). Secrets belong in the credential flow. |
| `--env-file <FILE>`          |                  | Read `KEY=VALUE` lines from a file into the workload env (repeatable; later files and `-e` win). |
| `-v`, `--volume`, `--mount <SPEC>` |            | Mount into the workload (repeatable): a named volume `name:/path[:ro]` (persists across runs) or a host bind `/host/path:/path[:ro]` (live host files; prompts to keep/drop secret-shaped files). Also accepts Docker keyed syntax: `type=bind\|volume,source=...,target=...[,readonly]`. |
| `-p`, `--publish <SPEC>`     |                  | Publish a guest port as `[host_ip:]hostport:containerport[/proto]` (repeatable). Host bind defaults to `127.0.0.1`. On a container-port conflict with declared `spec.ports`, `-p` wins. |
| `-i`, `--interactive`        | `true`           | Keep stdin open and forward host stdin to the workload. Disable with `--interactive=false` (or `-i=false`). |
| `-t`, `--tty`                | `true`           | Allocate a PTY; pipe mode is auto-selected when stdin isn't a TTY. Disable with `--tty=false` (or `-t=false`). |
| `-d`, `--detach`             | `false`          | Return immediately; the run continues in the service. Conflicts with `-i`/`-t`. |
| `--rm`                       | `false`          | Remove the run's state — record and writable layer — the moment the workload exits (Docker-style `--rm`). Without it every run persists as a stopped run, restartable with `lns start` until `lns rm`. |
| `--detach-keys <CHORD>`      | `ctrl-p,ctrl-q`  | Detach chord (single chars or `ctrl-X`, comma-separated). On match `lns` returns `0` and leaves the run executing in the background — re-join with `lns attach`; no signal is sent. Killing `lns` without the chord cancels the run. |
| `-u`, `--user <USER[:GROUP]>`| image `USER`, else `sandbox` | Run-as user or uid inside the sandbox; a numeric segment is used as the uid. Outranks the definition's `spec.user`, which outranks the image's `USER`. `HOME` and `USER` follow that user's guest passwd entry unless the definition's `env:` or a `-e` declares them, which wins; an image's `ENV HOME` does not. |
| `--entrypoint <COMMAND>`     | image `ENTRYPOINT` | Override the image `ENTRYPOINT`; the `COMMAND` after the reference is kept as its arguments. Pass `--entrypoint ""` to clear the image entrypoint. |
| `-h`, `--hostname <NAME>`    |                  | Set the guest hostname for this run.                                    |
| `-q`, `--quiet`              | `false`          | Suppress the launch banner and `✓` status lines; warnings, errors, and the workload's own output still print. |
| `[COMMAND...]`               |                  | Override the image command (and, with `--entrypoint`, its arguments). Accepted after the reference directly or after a `--` separator. |

`--cpus`, `--mem`, and `--registry` fall back to defaults stored with
[`lns config`](#lns-config); a per-run flag overrides its configured default.
For the environment, the layering is `--env-file` < `-e`.

See [Running workloads](running-workloads.md).

## `lns artifact`

Author, publish, fetch, and cache documents of any kind. Every verb here works on
a `sandbox` and on a `mixin` alike, because each reads the kind from the document
itself — there is no `lns mixin push`.

```bash
lns artifact init [--kind <sandbox|mixin>] [-f <FILE>]
lns artifact validate [--kind <KIND>] [-f <FILE>]
lns artifact push <REF> [--dry-run] [--yes] [-f <FILE>]
lns artifact pull <REF> [--yes]
lns artifact tag <SOURCE> <TARGET>
lns artifact ls [--kind <sandbox|mixin>] [--format <table|json>]
lns artifact inspect [REF] [--mixin <REF>]... [-f <FILE>]
lns artifact rm <REF>
lns artifact prune [-f]
```

`REF` is a registry coordinate (`ghcr.io/team/hermes:1.4.0`, `you/agent`,
`name@sha256:…`) or a path to a local document (`.`, `./dir`, `lns.dev.yaml`). A
bare reference is qualified by `run.registry`, else the Lens hub (`hub.lns.run`).

| Subcommand | Shortcut     | Meaning |
| ---------- | ------------ | ------- |
| `init`     | `lns init`   | Scaffold a document in this directory. `--kind` chooses which — `sandbox` (the default) or `mixin`; the file is `./lns.yaml` unless `-f`/`--file` names another. Refuses to overwrite. |
| `validate` | —            | Check the document named by `-f`, or `./lns.yaml`, offline — schema, cross-field, and secret checks — and list every problem, not just the first. It answers for whichever kind the file declares. Exits non-zero when the document is broken. `--kind <KIND>` also requires the document to be that kind. |
| `push`     | `lns push`   | Build `./lns.yaml` and upload it to a registry as one artifact — a `kind: sandbox` document publishes as a sandbox, a `kind: mixin` one as a mixin — in one step. Each `spec.filesets` `path` directory is packed into a layer of that same artifact, in declaration order, so the files and the declaration that mounts them share one digest; a `README.md` beside the document is packed into a `text/markdown` layer (max 1 MiB) the registry UI renders; each fuzzy `spec.tools` version resolves against the tool's public version index and publishes as an exact pin. A `spec.mixins` entry that names a local path is published too: the document it names goes up first as its own artifact, beside `<REF>` and under that mixin's own `name`, tagged with its own digest, and the entry you uploaded is pinned to that digest — your own file keeps the path. Push lists those mixins and asks before it uploads anything; `--yes` accepts without prompting. `--dry-run` validates, packs, and builds all of it offline, prints the digests every artifact would publish under, and uploads nothing (declared tools are not resolved — it notes when a published digest may differ). `-f`/`--file` publishes another file instead. |
| `pull`     | `lns pull`   | Inspect and fetch a published artifact and its base image into the local cache. A published **mixin** pulls too: it is config-only, so the pull caches its document and every mixin it names, which is what lets a digest-pinned graph resolve offline afterwards — the whole graph is recorded in the local store, so `lns artifact ls` lists it and `rm`/`prune` can reclaim it. If a **sandbox** declares tools, disclose them and ask before running their installers in a disposable provisioning guest — a mixin pull installs nothing, so it asks nothing; `--yes` accepts them non-interactively. The pull is bound to the inspected digest. |
| `tag`      | `lns tag`    | Re-reference a cached artifact under a new tag (`docker tag`-style). A bare `<TARGET>` follows the qualified `<SOURCE>`'s registry, so a same-repo retag never becomes a cross-registry pair. |
| `ls`       | —            | List what the local store holds — reference, kind, digest, size, and the sandbox holding each. `--kind` filters to `sandbox` (a pulled or built artifact) or `mixin` (a pulled mixin document). The base OCI images a sandbox runs on are cache-internal: they never list, and `prune` reclaims them silently once nothing needs them. Alias: `list`. |
| `inspect`  | `lns inspect`| Render one artifact's resolved content. With no operand, a path-shaped one (`.`, `lns.yaml`, `./dir`, `./lns.dev.yaml`), or `-f`/`--file`: that local document's effective form, offline — a `mixin` renders as one, not as a broken sandbox. For a cached reference: the artifact's kind and definition — a `sandbox`'s image, workdir, mounts, declared ports, filesets (`fileset: <source> -> <guestPath>`), connectors, declared tools (`tool: node@22.11.0`), its `pre-start` scripts with the user each asks for and its body printed whole, the mixins it resolved into (`mixin: <ref>`), and any over-broad-policy flag; a `mixin`'s own blocks as its author wrote them, unresolved; or a plain `image`. `--mixin <REF>` resolves that mixin in first (repeatable), so a composition can be previewed without starting a run. |
| `rm`       | `lns rm`     | Remove one cached artifact and free its now-unreferenced layers. |
| `prune`    | —            | Remove every cached artifact nothing holds and, when no sandbox is live, reclaim the provisioned tool cache. Lists what it would remove and asks first, unless `-f`/`--force`. |

The `./lns.yaml` document (`apiVersion: lns.run/v1`, `kind: sandbox`) carries a
`spec` with `image` (**required** base OCI image), and the optional `command`,
`workdir`, `volumes`, `env`, `egress`, and `connectors`. Declarative mounts
accept `type: bind` or `type: volume`, `source`, an absolute `target`, and optional
`readOnly`; explicit run mounts replace declarations with the same target. See
[Running workloads — defining a sandbox](running-workloads.md#defining-a-sandbox).

## `lns sandbox`

Create, run, watch, and remove sandboxes. A `RUN` is a sandbox's numeric id (`7`)
or its name (`reviewer`) — the two are interchangeable everywhere one is addressed.

```bash
lns sandbox run [OPTIONS] [REF] [-- COMMAND...]
lns sandbox exec [OPTIONS] <RUN> [-- COMMAND...]
lns sandbox kill <RUN> [--signal <SIG>]
lns sandbox stop <RUN> [-t <SECONDS>]
lns sandbox start <RUN> [-a [-i]] [--detach-keys <CHORD>]
lns sandbox logs [-f] <RUN>
lns sandbox attach <RUN> [--detach-keys <CHORD>]
lns sandbox ls [-a] [--format <table|json>]
lns sandbox inspect <RUN> [--format <table|json>]
lns sandbox rm <RUN> [-f]
lns sandbox prune [-f]
```

A sandbox outlives its workload. When the workload exits — or you `lns stop` it —
the sandbox stays listed as **stopped**, keeping its name and its writable layer,
until you remove it. `lns run --rm` opts out: that sandbox goes the moment its
workload does.

| Subcommand | Shortcut     | Meaning |
| ---------- | ------------ | ------- |
| `run`      | `lns run`    | Run a sandbox in a microVM. See [`lns run`](#lns-run). |
| `exec`     | `lns exec`   | Run another command inside a running sandbox. Stdin and PTY allocation are explicit: `-i` forwards stdin, `-t` allocates a PTY, and `-it` does both. `--detach-keys` closes only that exec session; `-q` suppresses status lines. The command needs no `--` separator. |
| `kill`     | `lns kill`   | Send one signal (`--signal`, default `TERM`; bare or `SIG`-prefixed, case-insensitive: `TERM`, `INT`, `QUIT`, `HUP`, `WINCH`, `KILL`) and return. |
| `stop`     | `lns stop`   | Stop a sandbox gracefully: SIGTERM first, SIGKILL once the timeout passes (`-t`, default 10s). Reports whether it had to escalate. The sandbox stays listed as **stopped**, restartable with `lns start` until you `lns sandbox rm` it. |
| `start`    | `lns start`  | Run a stopped sandbox again on its preserved writable layer. The launch replays exactly as recorded — image (digest-pinned), command, env, mounts, ports, resources, run-as — while the network rules and credentials re-resolve as they would for a fresh boot. Detached by default: prints the handle and returns. `-a` attaches output and adopts the workload's exit code; `-i` (with `-a`) forwards stdin. A conflict — a taken host port, a volume another sandbox holds, a missing bind source — aborts the start and leaves the sandbox stopped, untouched. |
| `logs`     | `lns logs`   | Print the sandbox's captured stdout/stderr; `-f` keeps streaming until the workload exits. The service keeps the most recent 2 MiB per sandbox. |
| `attach`   | `lns attach` | Re-join a sandbox's live output, most useful after `lns run -d`. The detach chord (`ctrl-p,ctrl-q` by default) leaves it running and returns you to your shell (docker-attach style; no signal is sent). Stdin reaches the workload only if the sandbox was started with stdin open. |
| `ls`       | `lns ps`     | List running sandboxes with their state, CPU, and memory. `-a`/`--all` includes the stopped ones; a stopped sandbox has no guest to sample, so its CPU and memory read `-`. Alias: `list`. |
| `inspect`  | `lns inspect`| Print one sandbox's live state and launch configuration. `--format <table\|json>` chooses the shape: `table` summarises it, `json` carries the whole launch configuration and the resolved policy. |
| `rm`       | `lns rm`     | Remove a sandbox: its record and its writable layer go together, the name frees up, and the artifact it held is released. Refuses a running one; `-f`/`--force` stops it first. |
| `prune`    | —            | Remove every stopped sandbox, writable layers included. Lists them and asks first, unless `-f`/`--force`. |

## `lns volume`

Manage the named volumes used with `lns run -v` (`docker volume`-style).

```bash
lns volume ls [--format <table|json>]
lns volume create <NAME>
lns volume inspect <NAME> [--format <table|json>]
lns volume rm <NAME>
lns volume prune [-f]
```

| Subcommand       | Meaning                                                                              |
| ---------------- | ------------------------------------------------------------------------------------ |
| `ls`             | List named volumes with their on-disk size, age, and the run holding them (if any).  |
| `create <NAME>`  | Create a named volume ahead of its first `lns run -v` attach. No-op if it exists.    |
| `inspect <NAME>` | Show a volume's capacity, on-disk bytes, age, and holder.                            |
| `rm <NAME>`      | Remove a volume and its data; refused while a run holds it.                          |
| `prune`          | Remove every volume not attached to a running sandbox. Lists them and asks first, unless `-f`/`--force`.|

See [Running workloads — volumes](running-workloads.md#volumes).

## `lns login` / `lns logout`

Store credentials for a private OCI registry so `lns run` and `lns pull` can fetch
its images. The background service keeps the credential store: a login is verified
against the registry and then saved by the service, and `lns logout` and
`lns login --list` go through it too, so the service must be running. Multiple
registries can be logged in at once.

```bash
lns login
echo "$TOKEN" | lns login -u <USERNAME> --password-stdin <REGISTRY>
lns login --list [--format <table|json>]
lns logout <REGISTRY>
```

A plain `lns login` — no credential flags — uses browser login on registries
that offer it (the Lens hub does): it prints a one-time confirmation code, opens
your browser to the registry's authorize page, and waits for you to approve the
login there; the registry then issues a fresh token for your account, which is
verified and stored like any other credential. Registries without browser
login (such as `ghcr.io`) still take the flag-driven forms below.

| Form                                  | Meaning                                                                 |
| ------------------------------------- | ----------------------------------------------------------------------- |
| `lns login [REGISTRY]`                | Log in to `REGISTRY` (defaults to `run.registry`, else `hub.lns.run`). With no credential flags, runs the browser login; pass `-u`/`--username` and the secret via `--password-stdin` (recommended) or `-p`/`--password` to log in with an existing token. |
| `lns login --list`                    | List the registries you are logged in to, as a `REGISTRY`/`USERNAME` table — never secrets. Takes `--format <table\|json>`. |
| `lns logout [REGISTRY]`               | Remove the stored credential for `REGISTRY`.                            |

The registry is matched by host: a bare published-sandbox reference uses the
`run.registry` default (or the Lens hub), while a fully-qualified
`lns run ghcr.io/org/app` always targets that registry and uses its stored login if present. Credentials live in
a per-user file (`~/.lns/registry-auth.json`, `0600`), separate from any
shareable policy.

## `lns audit`

Show one chronological timeline of every audit event across all sandboxes — or scope it to one.

```bash
lns audit                                   # every event, every sandbox, newest first
lns audit <sandbox>                         # scope to one sandbox: run id or unique id prefix
lns audit [--connector <id>] [--kind <kind>] [--format <table|jsonl>]
```

`lns audit` merges two sources into a single newest-first timeline: the per-run audit
logs (launch, egress, injected env, volume/bind mounts) and the durable connection
ledger (`approval`, `connection`, and `credential` events recorded across runs).
`<sandbox>` narrows it to one run — resolved as a run id or a unique id prefix; an
unknown sandbox prints `No audit events for sandbox …` and exits `0`.

Filters compose:

- `--connector <id>` — only events for one connector. Discover the ids with
  `lns connector list`; they also appear in the `DETAIL` column. Per-run egress/mount
  events carry no connector, so this narrows the stream to ledger events.
- `--kind <kind>` — one of `launch`, `egress`, `env`, `volume`, `bind`, `approval`,
  `connection`, `credential`, `tool`.
- `--format jsonl` — one raw JSON event per line instead of the table. (`--json` is the
  older spelling of the same thing and still works.)

Integrity is checked automatically as the log is read: if a hash chain has been altered,
truncated, or can't be verified against its anchor, `lns audit` prints an inline
`audit integrity:` warning and still lists what's there — the warning marks it
untrustworthy. There is no separate verify step. See [Audit](audit.md).

## `lns service`

Manage the background service.

```bash
lns service start | stop | status | enable | disable
```

| Subcommand | Meaning                                                                 |
| ---------- | ----------------------------------------------------------------------- |
| `start`    | Start the background service and wait until it's ready.                 |
| `stop`     | Stop the background service.                                            |
| `status`   | Show whether it's running (PID, uptime, version).                       |
| `enable`   | Register a per-user login agent and start the service now and on every login. |
| `disable`  | Stop the service and unregister the per-user login agent.               |

See [The background service](service.md).

## `lns update`

Update `lns` and `lns-service` to the latest release.

```bash
lns update [--force] [--dry-run]
```

| Option      | Default | Meaning                                                                        |
| ----------- | ------- | ------------------------------------------------------------------------------ |
| `--force`   | `false` | Reinstall even if the running version matches (e.g. corrupt or unsigned binary).|
| `--dry-run` | `false` | Print the anonymous update-check payload that would be sent (install ID, version, OS/arch) and exit without contacting the network. |

## `lns connector`

Manage the credential-connector catalog — the services whose credentials reach a
workload. The catalog is machine-global (`~/.lns/connectors.yaml`). A connector
declared in a sandbox definition's `spec.connectors` seeds its placeholder env
var but is only offered — the workload is prompted on first use, never armed
automatically. Connecting one records the connection for that project on this
machine and binds its value; the workload
still meets a first-use card, and answering it is what grants that workload the
value.

```bash
lns connector add <ID> --env-var <VAR> --inject <KIND:DOMAIN>... [--route <HOST>]... [--placeholder <P>]
lns connector list
lns connector remove <ID>
lns connector connect <ID> [--policy <PATH>]
lns connector disconnect <ID> [--policy <PATH>]
lns connector grants [--policy <PATH>] [--all]
lns connector revoke <ID> [--policy <PATH>]
```

| Subcommand   | Meaning                                                                       |
| ------------ | ----------------------------------------------------------------------------- |
| `add`        | Declare a credential connector in your machine-global catalog.              |
| `list`       | List the bundled and user-declared connectors and their auth kind.          |
| `remove`     | Remove a user-declared connector; bundled ones cannot be removed.           |
| `connect`    | Bind a connector's per-machine value decision: a credential connector prompts in the approval window (use the host value, store one, or deny) and an `oauth` connector signs in. Also records the id in this directory's policy — the bind path for ids a definition declares. |
| `disconnect` | Disconnect a connector from this directory's policy, forgetting its per-workload grants here. The grants go first, so a run that cannot update them leaves the connector connected to retry rather than stranding grants a later reconnect would inherit. |
| `grants`     | List the per-workload grants remembered for this project as `workload  connector  verdict`; `--all` adds a project column and covers every project on this machine. |
| `revoke`     | Forget one connector's per-workload grants in this project, so its next use asks again; exits `1` when there is nothing to forget. |

`--inject KIND:DOMAIN` is repeatable; `KIND` is `bearer_header`, `uri_placeholder`,
`token_header`, `basic_x_access_token`, or `api_key_header` (which takes the header
name as a third segment: `api_key_header:DOMAIN:HEADER`). Value decisions for a
connected connector are made interactively in the approval window; grants are
recorded per project and workload in `~/.lns/workload-grants.json`. See
[Credentials](credentials.md) and [Connectors](connectors.md).

## `lns config`

Get and set persistent defaults — set-once gap-fillers that apply to `lns run`
whenever the matching per-run flag is absent.

```bash
lns config set <KEY> <VALUE>...
lns config get <KEY>
lns config unset <KEY>
lns config list
```

| Subcommand | Meaning                                                                       |
| ---------- | ----------------------------------------------------------------------------- |
| `set`      | Set a default; each value is validated like the matching `lns run` flag.       |
| `get`      | Print a default's value(s), one per line; exits `1` when the key is not set.   |
| `unset`    | Remove a default.                                                              |
| `list`     | List every configured default as `key = value` lines.                          |

| Key           | Matching flag | Value form                                                |
| ------------- | ------------- | ---------------------------------------------------------- |
| `run.cpus`    | `--cpus`      | Number of vCPUs.                                           |
| `run.mem`     | `--mem`       | RAM in MiB.                                                |
| `run.registry`| `--registry`  | Default registry host for bare published-sandbox references (e.g. `ghcr.io`); unset means the Lens hub, `hub.lns.run`. |

The settable defaults are `run.cpus`, `run.mem`, and `run.registry`. Environment
variables, volumes, and ports are properties of a sandbox, not persistent config —
set them per run (`-e`, `-v`, `-p`) or in the sandbox definition's `spec`.

Values are validated when stored, with the same parsers the run flags use.
Defaults live in `~/.lns/config.yaml`, with everything else lns keeps for you.
A per-run flag always wins, and so do the resources a document declares in
`spec.resources` — a configured default only fills what no one else decided.
