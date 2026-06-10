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

## `lns sandbox`

Manage running sandboxes: stop, logs, attach, inspect, stats.

```bash
lns sandbox stop <RUN_ID> [-t <SECONDS>]
lns sandbox logs [-f] <RUN_ID>
lns sandbox attach <RUN_ID> [--detach-keys <CHORD>]
lns sandbox inspect <RUN_ID>
lns sandbox stats <RUN_ID>
```

| Subcommand | Meaning |
| ---------- | ------- |
| `stop`     | Stop a run gracefully: SIGTERM first, SIGKILL once the timeout passes (`-t`, default 10s). Reports whether it had to escalate. |
| `logs`     | Print the run's captured stdout/stderr; `-f` keeps streaming until the run exits. The service keeps the most recent 2 MiB of output per run, and only while the run is listed by `lns ls`. |
| `attach`   | Re-join a run's live output, most useful after `lns run -d`. The detach chord (`ctrl-p,ctrl-q` by default) behaves exactly like `lns run`'s. Stdin reaches the workload only if the run was started with stdin open. |
| `inspect`  | Print the run's state and launch configuration as JSON, with the policy file's parsed contents embedded when it is readable. |
| `stats`    | Sample the sandbox's CPU share and memory over one second, via the guest's `/proc`. |

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
