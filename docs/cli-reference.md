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
| `--cpus <N>`                 | `1`              | Number of vCPUs; falls back to the `run.cpus` config default.           |
| `--mem <MiB>`                | `512`            | RAM in MiB; falls back to the `run.mem` config default.                 |
| `--policy <PATH>`            | `lns-policy.yaml`| Policy file; auto-created with `defaultVerdict: ask` if absent.         |
| `-e`, `--env <KEY=VALUE>`    |                  | Set a non-secret environment variable (repeatable). Secrets belong in the credential flow. |
| `-v`, `--volume <SPEC>`      |                  | Attach a named volume as `name:/path[:ro]` (repeatable); persists across runs. |
| `-p`, `--publish <SPEC>`     |                  | Publish a guest port as `[host_ip:]hostport:containerport[/proto]` (repeatable). Host bind defaults to `127.0.0.1`. |
| `-i`, `--interactive`        | `true`           | Keep stdin open and forward host stdin to the workload.                 |
| `-t`, `--tty`                | `true`           | Allocate a PTY; pipe mode is auto-selected when stdin isn't a TTY.      |
| `-d`, `--detach`             | `false`          | Return immediately; the run continues in the service. Conflicts with `-i`/`-t`. |
| `--detach-keys <CHORD>`      | `ctrl-p,ctrl-q`  | Comma-separated detach chord (single chars or `ctrl-X`).                |
| `--sandbox-user <NAME>`      | `sandbox`        | Username the workload runs as inside the guest.                         |
| `--sandbox-uid <UID>`        | `65534`          | UID the workload runs as inside the guest.                              |
| `-- <COMMAND...>`            |                  | Override the entrypoint and command. Everything after `--`.             |

`--cpus`, `--mem`, `-e`, `-v`, and `-p` fall back to defaults stored with
[`lns config`](#lns-config); a per-run flag overrides its configured default.

See [Running workloads](running-workloads.md).

## `lns exec`

Open a new session against a running run (`docker exec`-style).

```bash
lns exec [OPTIONS] <RUN_ID> -- <COMMAND...>
```

| Option                  | Default         | Meaning                                                       |
| ----------------------- | --------------- | ------------------------------------------------------------- |
| `-i`, `--interactive`   | `true`          | Keep stdin open.                                              |
| `-t`, `--tty`           | `true`          | Allocate a PTY for the session.                               |
| `--detach-keys <CHORD>` | `ctrl-p,ctrl-q` | Detach chord; closes only this session, leaving the run alive.|
| `-- <COMMAND...>`       |                 | Command to run. Required. Everything after `--`.              |

## `lns kill`

Send a signal to a running run (`docker kill`-style).

```bash
lns kill <RUN_ID> [--signal <SIG>]
```

| Option            | Default | Meaning                                                                 |
| ----------------- | ------- | ----------------------------------------------------------------------- |
| `--signal <SIG>`  | `TERM`  | Signal name, bare or `SIG`-prefixed, case-insensitive. Supported: `TERM`, `INT`, `QUIT`, `HUP`, `WINCH`, `KILL`. |

## `lns ls`

List active runs (`docker ps`-style).

```bash
lns ls
```

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

## `lns audit`

Verify the audit chain of a run.

```bash
lns audit <RUN_ID>
```

`RUN_ID` is the identifier surfaced by `lns run` as `✓ started run #<id>`. Exits `0`
on an intact chain, non-zero if tampering is detected. See [Audit](audit.md).

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
