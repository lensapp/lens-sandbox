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
| `--cpus <N>`                 | `1`              | Number of vCPUs.                                                        |
| `--mem <MiB>`                | `512`            | RAM in MiB.                                                             |
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

## `lns audit`

Verify the audit chain of a run.

```bash
lns audit <RUN_ID>
```

`RUN_ID` is the identifier surfaced by `lns run` as `Started  run #<id>`. Exits `0`
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

## `lns credential`

Manage credential providers and their per-machine value decisions.

```bash
lns credential add  <ID> --env-var <VAR> --inject <KIND:DOMAIN>... [--placeholder <P>] [--value <V> | --value-stdin]
lns credential add-injection <ID> --inject <KIND:DOMAIN>
lns credential set  <ID> (--host | --value <V> | --value-stdin | --deny)
lns credential clear <ID>
lns credential list
lns credential remove <ID>
```

All subcommands except `clear` accept `--policy <PATH>` (default `lns-policy.yaml`).

| Subcommand      | Meaning                                                                  |
| --------------- | ------------------------------------------------------------------------ |
| `add`           | Declare a custom credential provider in the policy file.                 |
| `add-injection` | Add another per-domain injection to an existing custom provider.         |
| `set`           | Set a credential's value: the host value, a stored value, or deny.       |
| `clear`         | Clear a credential's value decision so the next use re-prompts.          |
| `list`          | List providers and their current value decisions.                        |
| `remove`        | Remove a custom provider (built-ins cannot be removed).                  |

Injection kinds declarable from the CLI: `bearer_header`, `uri_placeholder`. Prefer
`--value-stdin` over `--value` so secrets stay out of your shell history and the
process list. See [Credentials](credentials.md).
