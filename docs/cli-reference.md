# CLI reference

Every `lns` command, subcommand, and flag. Run `lns <command> --help` for the same
information at the terminal.

## Global options

| Option              | Default | Meaning                                                                              |
| ------------------- | ------- | ------------------------------------------------------------------------------------ |
| `--log-level <LVL>` | `warn`  | Log threshold. `warn` shows warnings/errors; `info` adds progress lines; `debug` adds traces and the guest boot transcript. Override with `LNS_LOG` or `RUST_LOG`. |
| `-V`, `--version`   |         | Print the version.                                                                   |
| `-h`, `--help`      |         | Print help. (`lns run` uses `-h` for `--hostname`; spell out `--help` there.)        |

Log levels: `error`, `warn`, `info`, `debug`.

## The sandbox surface

Lens Sandbox exposes a single noun — the **sandbox** — on two tiers:

- The **top level** carries the docker-familiar verbs. Every one of them (except
  `run`) is an exact shortcut into the `lns sandbox` namespace.
- **`lns sandbox <verb>`** is the complete surface, including the lns-native verbs
  that have no top-level shortcut.

| Top-level shortcut         | Expands to                        |
| -------------------------- | --------------------------------- |
| `lns run [REF]`            | `lns sandbox run [REF]`           |
| `lns init`                 | `lns sandbox init`                |
| `lns push <REF>`           | `lns sandbox push <REF>`          |
| `lns pull <REF>`           | `lns sandbox pull <REF>`          |
| `lns tag <SOURCE> <TARGET>`| `lns sandbox tag <SOURCE> <TARGET>` |
| `lns ps`                   | `lns sandbox ps`                  |
| `lns kill <RUN>`           | `lns sandbox kill <RUN>`          |
| `lns stop <RUN>`           | `lns sandbox stop <RUN>`          |
| `lns logs <RUN>`           | `lns sandbox logs <RUN>`          |
| `lns attach <RUN>`         | `lns sandbox attach <RUN>`        |
| `lns inspect <TARGET>`     | `lns sandbox inspect <TARGET>`    |
| `lns rm <REF>`             | `lns sandbox rm <REF>`            |

The lns-native verbs `validate`, `ls`, and `prune` live under `lns sandbox`
with no top-level shortcut. `lns exec` also works as an unlisted shorthand for
`lns sandbox exec`.

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
`REF`). A path-named definition's relative binds and filesets root at its own
directory, compose-style; the policy still comes from where you run. A
`COMMAND` after the reference overrides the sandbox base image's default command
(`lns run ghcr.io/acme/agent:1 echo hi`) while keeping its `ENTRYPOINT`; an explicit `--`
separator is still accepted. A command with no `REF` (`lns run -- echo hi`) runs
the `./lns.yaml` definition with its command overridden.

| Option                       | Default          | Meaning                                                                 |
| ---------------------------- | ---------------- | ----------------------------------------------------------------------- |
| `--cpus <N>`                 | `1`              | Number of vCPUs (at least 1); falls back to the `run.cpus` config default. |
| `-m`, `--mem`, `--memory <SIZE>` | `512`        | RAM in MiB, or with a unit suffix (`-m 2g`, `-m 512m`; rounded up to a whole MiB); falls back to the `run.mem` config default. |
| `-f`, `--file <FILE>`        | `./lns.yaml`     | Definition file to run instead of `./lns.yaml` (e.g. `lns.dev.yaml`); its directory roots the definition's relative binds and filesets. Cannot be combined with `REF`. |
| `--name <NAME>`              | auto             | Name the run, addressable by every `lns sandbox` verb in place of its id. Auto-generated (`adjective_noun`) when omitted; must not be all digits. |
| `--registry <HOST>`          | `docker.io`      | Registry to qualify a bare published-sandbox reference (e.g. `ghcr.io`); falls back to the `run.registry` config default. A fully-qualified reference is used as-is. |
| `--policy <PATH>`            | `lns-policy.yaml`| Policy file; auto-created with `defaultVerdict: ask` if absent.         |
| `-w`, `--workdir <DIR>`      | `spec.workdir`, then image `WORKDIR` | Working directory inside the sandbox (absolute path; created if missing). |
| `-e`, `--env <KEY=VALUE>`    |                  | Set a non-secret environment variable (repeatable). Secrets belong in the credential flow. |
| `--env-file <FILE>`          |                  | Read `KEY=VALUE` lines from a file into the workload env (repeatable; later files and `-e` win). |
| `-v`, `--volume`, `--mount <SPEC>` |            | Mount into the workload (repeatable): a named volume `name:/path[:ro]` (persists across runs) or a host bind `/host/path:/path[:ro]` (live host files; prompts to keep/drop secret-shaped files). Also accepts Docker keyed syntax: `type=bind\|volume,source=...,target=...[,readonly]`. |
| `-p`, `--publish <SPEC>`     |                  | Publish a guest port as `[host_ip:]hostport:containerport[/proto]` (repeatable). Host bind defaults to `127.0.0.1`. On a container-port conflict with declared `spec.ports`, `-p` wins. |
| `-P`, `--publish-declared`   | `false`          | Publish the definition's declared `spec.ports` on loopback (`host:` value when present, the container number otherwise). Automatic for a local `./lns.yaml` run; opt-in for a pulled sandbox. |
| `-i`, `--interactive`        | `true`           | Keep stdin open and forward host stdin to the workload. Disable with `--interactive=false` (or `-i=false`). |
| `-t`, `--tty`                | `true`           | Allocate a PTY; pipe mode is auto-selected when stdin isn't a TTY. Disable with `--tty=false` (or `-t=false`). |
| `-d`, `--detach`             | `false`          | Return immediately; the run continues in the service. Conflicts with `-i`/`-t`. |
| `--rm`                       | `false`          | Remove the run record once the workload exits (Docker-style `--rm`).    |
| `--detach-keys <CHORD>`      | `ctrl-p,ctrl-q`  | Detach chord (single chars or `ctrl-X`, comma-separated). On match `lns` returns `0` and leaves the run executing in the background — re-join with `lns attach`; no signal is sent. Killing `lns` without the chord cancels the run. |
| `-u`, `--user <USER[:GROUP]>`|                  | Run-as user or uid inside the sandbox. Alias for `--sandbox-user` / `--sandbox-uid`; a numeric segment is used as the uid. |
| `--sandbox-user <NAME>`      | image `USER`     | Username the workload runs as inside the guest (the unprivileged `sandbox` user when the image sets none). |
| `--sandbox-uid <UID>`        | image `USER` uid | UID the workload runs as inside the guest. |
| `--entrypoint <COMMAND>`     | image `ENTRYPOINT` | Override the image `ENTRYPOINT`; the `COMMAND` after the reference is kept as its arguments. Pass `--entrypoint ""` to clear the image entrypoint. |
| `-h`, `--hostname <NAME>`    |                  | Set the guest hostname for this run.                                    |
| `-q`, `--quiet`              | `false`          | Suppress the launch banner and `✓` status lines; warnings, errors, and the workload's own output still print. |
| `[COMMAND...]`               |                  | Override the image command (and, with `--entrypoint`, its arguments). Accepted after the reference directly or after a `--` separator. |

`--cpus`, `--mem`, and `--registry` fall back to defaults stored with
[`lns config`](#lns-config); a per-run flag overrides its configured default.
For the environment, the layering is `--env-file` < `-e`.

See [Running workloads](running-workloads.md).

## `lns sandbox`

The complete sandbox surface: author, distribute, run, and manage it.

```bash
# author (offline)
lns sandbox init
lns sandbox validate
lns inspect              # target-less: render ./lns.yaml, offline

# distribute
lns sandbox push <REF>
lns sandbox pull <REF>
lns sandbox tag <SOURCE> <TARGET>

# running lifecycle
lns sandbox ps
lns sandbox ls
lns sandbox exec [OPTIONS] <RUN> [-- COMMAND...]
lns sandbox kill <RUN> [--signal <SIG>]
lns sandbox stop <RUN> [-t <SECONDS>]
lns sandbox logs [-f] <RUN>
lns sandbox attach <RUN> [--detach-keys <CHORD>]
lns sandbox inspect <TARGET>

# cache
lns sandbox rm <REF>
lns sandbox prune [-f]
```

A `<REF>` addresses a **cached** sandbox (a registry coordinate); a `<RUN>`
addresses a **running** one by numeric id (`7`) or name (`reviewer`) — the two are
interchangeable everywhere a run is addressed.

| Subcommand | Shortcut       | Meaning |
| ---------- | -------------- | ------- |
| `init`     | `lns init`     | Scaffold a default `./lns.yaml` (`kind: Sandbox`) in this directory. |
| `validate` | —              | Validate `./lns.yaml` — schema, cross-field, and secret checks, offline. Exits non-zero and lists each problem when the definition is broken. `-f`/`--file` validates another definition file instead. |
| `push`     | `lns push`     | Build `./lns.yaml` and upload it to a registry as a sandbox artifact, in one step. `<REF>` is the registry reference to publish at. Each `spec.filesets` `path` directory is packed into a FileSet artifact, pushed alongside, and pinned by digest in the published config. `--dry-run` validates, packs, and builds all of it offline, prints the digests that would publish, and uploads nothing. `-f`/`--file` publishes another definition file instead. |
| `pull`     | `lns pull`     | Fetch a published sandbox and its base image into the local cache. |
| `tag`      | `lns tag`      | Re-reference a cached sandbox under a new tag (`docker tag`-style). |
| `ps`       | `lns ps`       | List running sandboxes with their CPU and memory (`docker ps`-style). |
| `ls`       | —              | List cached sandboxes (pulled or built) in the local store. Alias: `list`. |
| `exec`     | `lns exec`     | Open a new session against a running run (`docker exec`-style). `-i`/`-t`, `--detach-keys`, and `-q` work as for `lns run`; detaching closes only the exec session. |
| `kill`     | `lns kill`     | Send one signal (`--signal`, default `TERM`; bare or `SIG`-prefixed, case-insensitive: `TERM`, `INT`, `QUIT`, `HUP`, `WINCH`, `KILL`) and return. |
| `stop`     | `lns stop`     | Stop a run gracefully: SIGTERM first, SIGKILL once the timeout passes (`-t`, default 10s). Reports whether it had to escalate. |
| `logs`     | `lns logs`     | Print the run's captured stdout/stderr; `-f` keeps streaming until the run exits. The service keeps the most recent 2 MiB of output per run, while the run is listed. |
| `attach`   | `lns attach`   | Re-join a run's live output, most useful after `lns run -d`. The detach chord (`ctrl-p,ctrl-q` by default) leaves the run running and returns you to your shell (docker-attach style; no signal is sent). Stdin reaches the workload only if the run was started with stdin open. |
| `inspect`  | `lns inspect`  | With no target, a path-shaped one (`.`, `lns.yaml`, `./dir`, `./lns.dev.yaml`), or `-f`/`--file`: render that local definition's effective form, offline. For a running run: print its live state and launch configuration as JSON, with the policy file's parsed contents embedded when readable. For a cached reference: print the artifact's kind and definition — a `Sandbox`'s image, workdir, mounts, declared ports, filesets (`fileset: <ref> -> <mountPath>`), integrations, and any over-broad-policy flag; or a plain `Image`. |
| `rm`       | `lns rm`       | Remove a cached sandbox and free its now-unreferenced layers; refuses a running one (a running id/name is rejected). |
| `prune`    | —              | Remove every cached sandbox not held by a running one, reclaiming disk. Requires `-f`/`--force` — there is no interactive prompt. |

The `./lns.yaml` definition (`apiVersion: lns.run/v1`, `kind: Sandbox`) carries a
`spec` with `image` (**required** base OCI image), and the optional `command`,
`workdir`, `volumes`, `env`, `policy`, and `integrations`. Declarative mounts
accept `type: bind` or `type: volume`, `source`, an absolute `target`, and optional
`readOnly`; explicit run mounts replace declarations with the same target. See
[Running workloads — defining a sandbox](running-workloads.md#defining-a-sandbox).

## `lns volume`

Manage the named volumes used with `lns run -v` (`docker volume`-style).

```bash
lns volume ls | create <NAME> | inspect <NAME> | rm <NAME> | prune [-f]
```

| Subcommand       | Meaning                                                                              |
| ---------------- | ------------------------------------------------------------------------------------ |
| `ls`             | List named volumes with their on-disk size, age, and the run holding them (if any).  |
| `create <NAME>`  | Create a named volume ahead of its first `lns run -v` attach. No-op if it exists.    |
| `inspect <NAME>` | Show a volume's details as JSON (capacity, on-disk bytes, created, holder).          |
| `rm <NAME>`      | Remove a volume and its data; refused while a run holds it.                          |
| `prune`          | Remove every volume not attached to a running sandbox. Prompts unless `-f`/`--force`.|

See [Running workloads — volumes](running-workloads.md#volumes).

## `lns login` / `lns logout`

Store credentials for a private OCI registry so `lns run` and `lns pull` can fetch
its images. Credentials are verified against the registry before they are saved
(the background service must be running), and multiple registries can be logged in
at once.

```bash
echo "$TOKEN" | lns login -u <USERNAME> --password-stdin <REGISTRY>
lns login --list
lns logout <REGISTRY>
```

| Form                                  | Meaning                                                                 |
| ------------------------------------- | ----------------------------------------------------------------------- |
| `lns login [REGISTRY]`                | Log in to `REGISTRY` (defaults to `run.registry`, else `docker.io`). Pass `-u`/`--username` and the secret via `--password-stdin` (recommended) or `-p`/`--password`. |
| `lns login --list`                    | List the registries you are logged in to, as `host  username` — never secrets. |
| `lns logout [REGISTRY]`               | Remove the stored credential for `REGISTRY`.                            |

The registry is matched by host: a bare published-sandbox reference uses the
`run.registry` default (or Docker Hub), while a fully-qualified
`lns run ghcr.io/org/app` always targets that registry and uses its stored login if present. Credentials live in
a per-user file (`~/.lns-registry-auth.json`, `0600`; override with
`LNS_REGISTRY_AUTH_PATH`), separate from any shareable policy.

## `lns audit`

Show one chronological timeline of every audit event across all sandboxes — or scope it to one.

```bash
lns audit                                   # every event, every sandbox, newest first
lns audit <sandbox>                         # scope to one sandbox: run id or unique id prefix
lns audit [--integration <id>] [--kind <kind>] [--json]
```

`lns audit` merges two sources into a single newest-first timeline: the per-run audit
logs (launch, egress, injected env, volume/bind mounts) and the durable connection
ledger (`approval`, `connection`, and `credential` events recorded across runs).
`<sandbox>` narrows it to one run — resolved as a run id or a unique id prefix; an
unknown sandbox prints `No audit events for sandbox …` and exits `0`.

Filters compose:

- `--integration <id>` — only events for one integration. Discover the ids with
  `lns integration list`; they also appear in the `DETAIL` column. Per-run egress/mount
  events carry no integration, so this narrows the stream to ledger events.
- `--kind <kind>` — one of `launch`, `egress`, `env`, `volume`, `bind`, `approval`,
  `connection`, `credential`.
- `--json` — one raw JSON event per line instead of the table.

Integrity is checked automatically as the log is read: if a hash chain has been altered,
truncated, or can't be verified against its anchor, `lns audit` prints an inline
`audit integrity:` warning and still lists what's there — the warning marks it
untrustworthy. There is no separate verify step. See [Audit](audit.md).

## `lns service`

Manage the background service.

```bash
lns service start | stop | restart | status | enable | disable
```

| Subcommand | Meaning                                                                 |
| ---------- | ----------------------------------------------------------------------- |
| `start`    | Start the background service and wait until it's ready.                 |
| `stop`     | Stop the background service.                                            |
| `restart`  | Stop it, then start the currently installed binary.                     |
| `status`   | Show whether it's running (PID, uptime, version, build, protocol).      |
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

## `lns policy`

Edit network rules in a policy file. All subcommands accept `--policy <PATH>`
(default `lns-policy.yaml` in the current directory).

```bash
lns policy allow  <PATTERN> [--description <TEXT>]
lns policy deny   <PATTERN> [--description <TEXT>]
lns policy list
lns policy remove <PATTERN>
```

| Subcommand | Meaning                                                          |
| ---------- | ---------------------------------------------------------------- |
| `allow`    | Add an allow rule for a destination pattern.                     |
| `deny`     | Add a deny rule for a destination pattern.                       |
| `list`     | List the rules in the policy file.                               |
| `remove`   | Remove the rule matching a destination pattern.                  |

`PATTERN` is a host, wildcard (`*.github.com`), CIDR, or `host:port`. See
[Policy and approvals](policy.md).

## `lns integration`

Manage the credential-integration catalog — the services whose credentials reach a
workload. The catalog is machine-global (`~/.lns-integrations.yaml`). An integration
declared in a sandbox definition's `spec.integrations` is only offered — the
workload is prompted to connect it on first use, never armed automatically;
connecting one (here or reactively) arms it and records it under `integrations:`
in that directory's `lns-policy.yaml`.

```bash
lns integration add <ID> --env-var <VAR> --inject <KIND:DOMAIN>... [--route <HOST>]... [--placeholder <P>]
lns integration list
lns integration remove <ID>
lns integration connect <ID> [--policy <PATH>]
lns integration disconnect <ID> [--policy <PATH>]
```

| Subcommand   | Meaning                                                                       |
| ------------ | ----------------------------------------------------------------------------- |
| `add`        | Declare a credential integration in your machine-global catalog.              |
| `list`       | List the bundled and user-declared integrations and their auth kind.          |
| `remove`     | Remove a user-declared integration; bundled ones cannot be removed.           |
| `connect`    | Bind an integration's per-machine value decision: a credential integration prompts in the approval window (use the host value, store one, or deny) and an `oauth` integration signs in. Also records the id in this directory's policy — the bind path for ids a definition declares or requires. |
| `disconnect` | Disconnect an integration from this directory's policy.                       |

`--inject KIND:DOMAIN` is repeatable; `KIND` is `bearer_header`, `uri_placeholder`,
`token_header`, `basic_x_access_token`, or `api_key_header` (which takes the header
name as a third segment: `api_key_header:DOMAIN:HEADER`). Value decisions for a
connected integration are made interactively in the approval window — see
[Credentials](credentials.md) and [Integrations](integrations.md).

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
| `run.registry`| `--registry`  | Default registry host for bare published-sandbox references (e.g. `ghcr.io`). |

The settable defaults are `run.cpus`, `run.mem`, and `run.registry`. Environment
variables, volumes, and ports are properties of a sandbox, not persistent config —
set them per run (`-e`, `-v`, `-p`) or in the sandbox definition's `spec`.

Values are validated when stored, with the same parsers the run flags use.
Defaults live in a per-user file — `~/Library/Application Support/lns/config.yaml`
on macOS, `~/.config/lns/config.yaml` on Linux; override with `LNS_CONFIG_PATH`.
A per-run flag always wins.
