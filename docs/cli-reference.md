# CLI reference

Every `lns` command, subcommand, and flag. Run `lns <command> --help` for the same
information at the terminal.

## Global options

| Option              | Default | Meaning                                                                              |
| ------------------- | ------- | ------------------------------------------------------------------------------------ |
| `--log-level <LVL>` | `warn`  | Log threshold. `warn` shows warnings/errors; `info` adds progress lines; `debug` adds traces and the guest boot transcript. Override with `LNS_LOG` or `RUST_LOG`. |
| `--version`         |         | Print the version.                                                                   |
| `--help`            |         | Print help.                                                                          |

Log levels: `error`, `warn`, `info`, `debug`.

## `lns run`

Run an OCI image in a microVM.

```bash
lns run [OPTIONS] [IMAGE] [-- COMMAND...]
```

`IMAGE` is an OCI image reference (e.g. `alpine:3.20`); omit it for an imageless
run, which requires a `COMMAND` after `--`.

| Option                       | Default          | Meaning                                                                 |
| ---------------------------- | ---------------- | ----------------------------------------------------------------------- |
| `--cpus <N>`                 | `1`              | Number of vCPUs (at least 1); falls back to the `run.cpus` config default. |
| `-m`, `--mem`, `--memory <SIZE>` | `512`        | RAM in MiB, or with a unit suffix (`-m 2g`, `-m 512m`; rounded up to a whole MiB); falls back to the `run.mem` config default. |
| `--name <NAME>`              | auto             | Name the run, addressable by every `lns sandbox` verb in place of its id. Auto-generated (`adjective_noun`) when omitted; must not be all digits. |
| `--registry <HOST>`          | `docker.io`      | Registry to qualify a bare image reference (e.g. `ghcr.io`); falls back to the `run.registry` config default. A fully-qualified reference is used as-is. |
| `--policy <PATH>`            | `lns-policy.yaml`| Policy file; auto-created with `defaultVerdict: ask` if absent.         |
| `-w`, `--workdir <DIR>`      | image `WORKDIR`  | Working directory inside the sandbox (absolute path; created if missing). |
| `-e`, `--env <KEY=VALUE>`    |                  | Set a non-secret environment variable (repeatable). Secrets belong in the credential flow. |
| `--env-file <FILE>`          |                  | Read `KEY=VALUE` lines from a file into the workload env (repeatable; later files and `-e` win). |
| `-v`, `--volume <SPEC>`      |                  | Mount into the workload (repeatable): a named volume `name:/path[:ro]` (persists across runs) or a host bind `/host/path:/path[:ro]` (live host files; prompts to keep/drop secret-shaped files). |
| `-p`, `--publish <SPEC>`     |                  | Publish a guest port as `[host_ip:]hostport:containerport[/proto]` (repeatable). Host bind defaults to `127.0.0.1`. |
| `-i`, `--interactive`        | `true`           | Keep stdin open and forward host stdin to the workload. Disable with `--interactive=false` (or `-i=false`). |
| `-t`, `--tty`                | `true`           | Allocate a PTY; pipe mode is auto-selected when stdin isn't a TTY. Disable with `--tty=false` (or `-t=false`). |
| `-d`, `--detach`             | `false`          | Return immediately; the run continues in the service. Conflicts with `-i`/`-t`. |
| `--detach-keys <CHORD>`      | `ctrl-p,ctrl-q`  | Detach chord (single chars or `ctrl-X`, comma-separated). On match `lns` returns `0` and leaves the run executing in the background — re-join with `lns sandbox attach`; no signal is sent. Killing `lns` without the chord cancels the run. |
| `--sandbox-user <NAME>`      | `sandbox`        | Username the workload runs as inside the guest.                         |
| `--sandbox-uid <UID>`        | `65534`          | UID the workload runs as inside the guest.                              |
| `-- <COMMAND...>`            |                  | Override the entrypoint and command. Everything after `--`.             |

`--cpus`, `--mem`, `-e`, `-v`, and `-p` fall back to defaults stored with
[`lns config`](#lns-config); a per-run flag overrides its configured default. For
the environment, the layering is `run.env` < `--env-file` < `-e`.

See [Running workloads](running-workloads.md).

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

## `lns image`

Manage the cached OCI images that `lns run` boots from (`docker image`-style).

```bash
lns image pull <IMAGE> | ls | rm <IMAGE> | prune [-f]
```

| Subcommand     | Meaning                                                                                  |
| -------------- | ---------------------------------------------------------------------------------------- |
| `pull <IMAGE>` | Pull an image into the cache ahead of `lns run`, printing its resolved digest and a pinnable `repo@sha256:…` reference. |
| `ls`           | List cached images with their digest, size, pull time, and the run using them (if any).  |
| `rm <IMAGE>`   | Remove a cached image and reclaim layers no other cached image shares; refused while a run uses it. |
| `prune`        | Remove every cached image not used by a running sandbox. Prompts unless `-f`/`--force`.  |

See [Running workloads — images](running-workloads.md#managing-the-image-cache).

## `lns login` / `lns logout`

Store credentials for a private OCI registry so `lns run` and `lns image pull`
can fetch its images. Credentials are verified against the registry before they
are saved (the background service must be running), and multiple registries can
be logged in at once.

```bash
echo "$TOKEN" | lns login -u <USERNAME> --password-stdin <REGISTRY>
lns login --list
lns logout <REGISTRY>
```

| Form                                  | Meaning                                                                 |
| ------------------------------------- | ----------------------------------------------------------------------- |
| `lns login [REGISTRY]`                | Log in to `REGISTRY` (defaults to `run.registry`, else `docker.io`). Pass `-u`/`--username` and the secret via `--password-stdin` (recommended) or `--password`. |
| `lns login --list`                    | List the registries you are logged in to, as `host  username` — never secrets. |
| `lns logout [REGISTRY]`               | Remove the stored credential for `REGISTRY`.                            |

The registry is matched by host: a bare `lns run alpine` uses the `run.registry`
default (or Docker Hub), while a fully-qualified `lns run ghcr.io/org/app` always
targets that registry and uses its stored login if present. Credentials live in
a per-user file (`~/.lns-registry-auth.json`, `0600`; override with
`LNS_REGISTRY_AUTH_PATH`), separate from any shareable policy.

## `lns sandbox`

Manage sandboxes: ls, exec, kill, stop, logs, attach, inspect, stats, rm, rename, prune.

```bash
lns sandbox ls [-a|--all]
lns sandbox exec [OPTIONS] <RUN> -- <COMMAND...>
lns sandbox kill <RUN> [--signal <SIG>]
lns sandbox stop <RUN> [-t <SECONDS>]
lns sandbox logs [-f] <RUN>
lns sandbox attach <RUN> [--detach-keys <CHORD>]
lns sandbox inspect <RUN>
lns sandbox stats <RUN>
lns sandbox rm <RUN>
lns sandbox rename <RUN> <NEW_NAME>
lns sandbox prune
```

`<RUN>` is a run's numeric id (`7`) or its name (`reviewer`) — the two are
interchangeable everywhere a run is addressed.

| Subcommand | Meaning |
| ---------- | ------- |
| `ls`       | List running runs by default (`docker ps`-style); `-a` / `--all` also shows finished runs from the durable run history. |
| `exec`     | Open a new session against a running run (`docker exec`-style). `-i`/`-t` and `--detach-keys` work as for `lns run`; detaching closes only the exec session. |
| `kill`     | Send one signal (`--signal`, default `TERM`; bare or `SIG`-prefixed, case-insensitive: `TERM`, `INT`, `QUIT`, `HUP`, `WINCH`, `KILL`) and return. |
| `stop`     | Stop a run gracefully: SIGTERM first, SIGKILL once the timeout passes (`-t`, default 10s). Reports whether it had to escalate. |
| `logs`     | Print the run's captured stdout/stderr; `-f` keeps streaming until the run exits. The service keeps the most recent 2 MiB of output per run while it still has that run in memory. |
| `attach`   | Re-join a run's live output, most useful after `lns run -d`. The detach chord (`ctrl-p,ctrl-q` by default) leaves the run running and returns you to your shell (docker-attach style; no signal is sent). Stdin reaches the workload only if the run was started with stdin open. |
| `inspect`  | Print the run's state and launch configuration as JSON, with the policy file's parsed contents embedded when it is readable. |
| `stats`    | Sample the sandbox's CPU share and memory over one second, via the guest's `/proc`. |
| `rm`       | Remove a single finished run record and its on-disk run directory (`docker rm`-style). Refuses a run that is still running — stop it first. |
| `rename`   | Give a run a name or change it (`docker rename`-style); the new name resolves immediately and must be unique among listed runs. |
| `prune`    | Remove every finished run record at once (`docker container prune`-style); running runs are left untouched. |

The pre-namespace spellings `lns ls`, `lns exec`, and `lns kill` keep working
as hidden aliases; the `lns sandbox` forms are the documented ones.

## `lns audit`

Verify the audit chain of a run.

```bash
lns audit <RUN_ID> [--allow-missing-anchor]
```

`RUN_ID` is the identifier surfaced by `lns run` as `✓ started run #<id>`. Exits `0`
on an intact chain, non-zero if tampering is detected, or if the anchor that guards
against truncation is missing, corrupt, or unreadable. `--allow-missing-anchor`
accepts a missing anchor (chain-only check) with exit `0`. See [Audit](audit.md).

## `lns service`

Manage the background service.

```bash
lns service start | stop | status
```

| Subcommand | Meaning                                  |
| ---------- | ---------------------------------------- |
| `start`    | Start the background service.            |
| `stop`     | Stop the background service.             |
| `status`   | Show whether it's running.               |

See [The background service](service.md).

## `lns update`

Update `lns` and `lns-service` to the latest release.

```bash
lns update [--force]
```

| Option    | Default | Meaning                                                                        |
| --------- | ------- | ------------------------------------------------------------------------------ |
| `--force` | `false` | Reinstall even if the running version matches (e.g. corrupt or unsigned binary).|

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
workload. The catalog is machine-global (`~/.lns-integrations.yaml`); connecting one
records it under `integrations:` in a directory's `lns-policy.yaml`.

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
| `connect`    | Connect an integration to this directory's policy (`oauth` integrations sign in). |
| `disconnect` | Disconnect an integration from this directory's policy.                       |

`--inject KIND:DOMAIN` is repeatable; `KIND` is `bearer_header`, `uri_placeholder`,
`token_header`, `basic_x_access_token`, or `api_key_header` (which takes the header
name as a third segment: `api_key_header:DOMAIN:HEADER`). Value decisions for a
connected integration are made interactively in the approval window — see
[Credentials](credentials.md) and [Integrations](integrations.md).

## `lns config`

Get and set persistent defaults — set-once settings that apply to `lns run`
whenever the matching per-run flag is absent.

```bash
lns config set <KEY> <VALUE>...
lns config get <KEY>
lns config unset <KEY>
lns config list
```

| Subcommand | Meaning                                                                       |
| ---------- | ----------------------------------------------------------------------------- |
| `set`      | Set a default; list keys take multiple values and replace all previous ones.   |
| `get`      | Print a default's value(s), one per line; exits `1` when the key is not set.   |
| `unset`    | Remove a default.                                                              |
| `list`     | List every configured default as `key = value` lines.                          |

| Key           | Matching flag | Value form                                                |
| ------------- | ------------- | ---------------------------------------------------------- |
| `run.cpus`    | `--cpus`      | Number of vCPUs.                                           |
| `run.mem`     | `--mem`       | RAM in MiB.                                                |
| `run.registry`| `--registry`  | Default registry host for bare image references (e.g. `ghcr.io`). |
| `run.env`     | `-e`          | `KEY=VALUE` (multiple values allowed).                     |
| `run.volume`  | `-v`          | `name:/path[:ro]` (multiple values allowed).               |
| `run.publish` | `-p`          | `[host_ip:]hostport:containerport[/proto]` (multiple values allowed). |

Values are validated when stored, with the same parsers the run flags use.
Defaults live in a per-user file — `~/Library/Application Support/lns/config.yaml`
on macOS, `~/.config/lns/config.yaml` on Linux; override with `LNS_CONFIG_PATH`.

A per-run flag always wins. For `run.cpus` and `run.mem` it replaces the
configured value outright; for the list keys precedence is entry-by-entry — a
per-run `-e` overrides the configured entry with the same variable name, `-v`
the configured mount at the same target, `-p` the configured publish on the
same host bind — and the remaining configured entries still apply.
