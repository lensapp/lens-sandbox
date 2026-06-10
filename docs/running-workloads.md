# Running workloads

Everything that runs inside a sandbox goes through `lns run`. Once a run is up you
can open extra sessions into it with `lns exec`, list active runs with `lns ls`,
and signal them with `lns kill`. The model is deliberately close to `docker`.

The background service must be running first (`lns service start`).

## `lns run`

```bash
lns run [OPTIONS] [IMAGE] [-- COMMAND...]
```

You run `lns run` from a project directory; that's where Lens Sandbox looks for the
`lns-policy.yaml` that governs the run. To expose host files to the workload, attach
a volume with `-v`.

### Running an image

```bash
cd ~/dev/my-app
lns run ghcr.io/acme/agent:latest
```

The image is pulled (and cached by the service) and its entrypoint starts inside
the microVM.

### Pinning an image by digest

A tag like `:latest` is mutable — the registry can republish it at any time. To run
exactly the bytes you vetted, pin the image by digest:

```bash
lns run ghcr.io/acme/agent@sha256:8a2c…
```

`lns image pull` resolves and prints the pinnable reference for you. Pinned
manifests are cached, so reruns of a pinned image don't re-consult the registry.

### Managing the image cache

Every pull lands in the service's content-addressed cache so the next run boots
without re-downloading. Manage it with `lns image`:

```bash
lns image pull ghcr.io/acme/agent:latest  # pre-warm + resolve a pinnable digest
lns image ls                              # cached images: digest, size, pull time, user
lns image rm ghcr.io/acme/agent:latest    # drop one image (refused while a run uses it)
lns image prune                           # drop every image no running sandbox uses
```

`rm` and `prune` never touch an image a live run is using, and only delete layers
that no remaining cached image shares. `prune` asks for confirmation unless you
pass `-f`/`--force`. Removing a cached image is always safe in the durable sense —
the next `lns run` simply pulls it again.

### Overriding the command

Anything after `--` replaces the image's entrypoint and command:

```bash
lns run alpine:3.20 -- sh -c 'echo hello && uname -a'
```

### Imageless runs

Omit the image to boot a minimal guest and run a command directly. A command after
`--` is required in this mode:

```bash
lns run -- /bin/sh
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

The workload starts in the image's `WORKDIR`, like `docker run`. Override it
with `-w` (an absolute guest path, created inside the sandbox if missing):

```bash
lns run -w /workspace ghcr.io/acme/agent
```

### Persistent defaults

Settings you'd otherwise repeat on every run can be stored once with
[`lns config`](cli-reference.md#lns-config). A per-run flag always overrides its
configured default:

```bash
lns config set run.cpus 4
lns config set run.mem 2048
lns run ghcr.io/acme/builder            # boots with 4 vCPUs · 2048 MiB
lns run --cpus 2 ghcr.io/acme/builder   # per-run flag wins: 2 vCPUs
```

Defaults exist for `run.cpus`, `run.mem`, `run.env`, `run.volume`, and
`run.publish`. For the list keys, a flag overrides only the conflicting
configured entry (same variable name, mount target, or host bind) — the rest
still apply.

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
host variables through implicitly would be a silent leak channel.

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

### Interactive, TTY, and detached sessions

| Flag                  | Default | Meaning                                                              |
| --------------------- | ------- | -------------------------------------------------------------------- |
| `-i`, `--interactive` | `true`  | Keep stdin open and forward host stdin to the workload.              |
| `-t`, `--tty`         | `true`  | Allocate a PTY. Pipe mode is selected automatically when stdin isn't a TTY. |
| `-d`, `--detach`      | `false` | Start the run and return immediately; it keeps running in the service.|

By default a run is attached to your terminal. Detach to leave it running in the
background:

```bash
lns run -d ghcr.io/acme/long-job
# prints: run #7
```

A detached run is reachable later via `lns exec`, `lns ls`, `lns kill`, and the
[`lns sandbox` lifecycle verbs](#lns-sandbox--managing-runs-youve-started).
`-d` cannot be combined with `-i`/`-t`.

### Detaching from an attached run

While attached, the detach chord (default `ctrl-p,ctrl-q`) leaves the run running
and returns you to your shell. Change it with `--detach-keys`:

```bash
lns run --detach-keys ctrl-x,ctrl-x ghcr.io/acme/agent
```

The value is a comma-separated chord of single characters or `ctrl-X` tokens.

## `lns exec`

Open another session inside a running run, like `docker exec`:

```bash
lns exec 7 -- bash
```

The first argument is the run id (shown by `lns run -d` and `lns ls`). `-i`, `-t`,
and `--detach-keys` work as they do for `lns run`. Detaching from an exec session
closes only that session — the run and any other sessions keep going.

This is also how you open a debugging shell alongside a misbehaving workload
without disturbing it.

## `lns ls`

List active runs:

```bash
lns ls
```

## `lns kill`

Send a signal to a run (default `SIGTERM`), like `docker kill`:

```bash
lns kill 7                 # SIGTERM
lns kill 7 --signal KILL   # SIGKILL
```

Signal names are case-insensitive and may be bare or `SIG`-prefixed. Supported
signals: `TERM`, `INT`, `QUIT`, `HUP`, `WINCH`, `KILL`.

For a graceful shutdown that waits and escalates for you, prefer
`lns sandbox stop`.

## `lns sandbox` — managing runs you've started

The `lns sandbox` family extends `ls`/`exec`/`kill` with the rest of the
lifecycle:

```bash
lns sandbox stop 7             # SIGTERM, wait up to 10s, then SIGKILL
lns sandbox stop 7 -t 30       # give it longer to clean up
lns sandbox logs 7             # print the captured output so far
lns sandbox logs -f 7          # ...and keep following until it exits
lns sandbox attach 7           # re-join a detached run live
lns sandbox inspect 7          # state + launch config as JSON
lns sandbox stats 7            # CPU share and memory, sampled over 1s
```

### Stopping vs killing

`lns kill` sends one signal and returns. `lns sandbox stop` owns the whole
shutdown: it sends `SIGTERM`, waits up to the timeout for the workload to exit,
and only then sends `SIGKILL`. The command reports which of the two happened —
`stopped run #7` for a graceful exit, `killed run #7` when it had to escalate.

### Logs

The service keeps a rolling capture of every run's stdout and stderr — the most
recent 2 MiB — for as long as the run is listed by `lns ls`. `lns sandbox logs`
prints what's buffered; `-f` streams new output until the run exits. Output of
`lns exec` sessions is not captured, only the run's primary session.

### Attaching

`lns sandbox attach` joins a run's output from now on (no history replay) and
forwards your keystrokes when the run was started with stdin open. The detach
chord works exactly as it does for `lns run`. Note that a run started with
`-d` has stdin closed, so attach is primarily a live view of its output.

### Inspecting

`lns sandbox inspect` prints one JSON document with the run's status, image,
command, and launch configuration (cpus, memory, env, ports, volumes, run-as
identity), plus the contents of its policy file when that file is readable on
this machine.

### Stats

`lns sandbox stats` samples `/proc` inside the guest over one second and
reports the sandbox's CPU share and memory use — the microVM is the workload,
so the numbers cover everything the run is doing.

## See also

- [Policy and approvals](policy.md) — control what the workload may reach.
- [Credentials](credentials.md) — give the workload placeholders, not secrets.
- [CLI reference](cli-reference.md) — the full flag list.
