# lns-sandbox — Lens Sandbox plugin for Claude Code

Route Claude Code's code-execution commands into [Lens Sandbox](https://lns.run) microVMs.
When the agent runs an untrusted interpreter or a download-and-run one-shot, the command is
intercepted and executed inside an `lns` microVM instead of on the host — with network and
credential access gated by lns policy (you approve egress and secrets via lns's prompts, the
same way a sandboxed `gh` asks for its token).

## How it works

A `PreToolUse` hook on the `Bash` tool inspects each command. If it's code-execution worth
isolating, the hook rewrites it to run inside a microVM:

```
python app.py
   │  PreToolUse hook (lns-cc hook)
   ▼
lns-cc exec python --b64 <base64("python app.py")>
   │  ephemeral backend (default), then …
   ▼
lns run --name cc-python-<sha8(cwd)> -v <cwd>:<cwd> -w <cwd> python:3.12 -- bash -lc 'python app.py'
```

The whole command runs in the guest against the project directory (bind-mounted at its real
absolute path), so `cd`, pipes, and compound commands all work. Read-only host inspection
(`ls`, `cat`, `git status`, …) is never intercepted.

### Backends

- **Ephemeral (default):** one `lns run` per command, stdin from `/dev/null`. Output is passed
  through a filter that strips the guest supervisor's tracing and `[agent]` markers — a workaround
  for [lensapp/lens-sandbox#94](https://github.com/lensapp/lens-sandbox/issues/94), which forces
  PTY mode (merging that noise onto stdout) because `-i`/`-t` can't be disabled.
- **Persistent (`LNS_CC_BACKEND=persistent`):** one long-lived box per `(cwd, runtime)`, exec'd
  into per command — faster after first boot, warm dependency caches, clean output with no filter.
  **Blocked until #94 lands** (non-interactive `lns sandbox exec` is currently rejected).

When #94 is fixed, persistent becomes the default and the stdout filter is removed.

The division of consent: Claude Code's permission decides *whether code may run*; lns's policy
decides *what that code may reach* (network hosts, credentials).

## What gets sandboxed (default scope)

**Tier A — always:**
- Download-and-execute one-shots: `curl … | bash`, `wget -O- … | sh`, `npx`, `pnpm dlx`,
  `bunx`, `uvx` / `uv run`, `pipx run`.
- Interpreters running scripts: `python`/`python3`, `node`, `ruby`, `deno`, `bun`, `php`, `perl`.
- `bash`/`sh` only when fed from a pipe (the `curl | bash` form), never a local script file.

**Tier B — opt-in:** package installs (recommended on), build systems and test runners (off).

**Tier C — opt-in:** network/auth CLIs (`gh`, `curl`/`wget` to APIs, `git push`) for the
policy + credential flow.

**Never:** read-only inspection, shell navigation, and `lns …` itself.

## Host access model

- **Project files** — bind-mounted; just work, at the same absolute path as the host.
- **Files outside the project** — granted read-only on demand (`/lns-sandbox grant <path>`)
  or via the `mounts:` config.
- **Environment variables** — none forwarded by default; an `env_forward:` allowlist passes
  non-secret vars.
- **Secrets** (`~/.aws`, `~/.ssh`, tokens) — never bind-mounted; routed through lns's
  credential/integration flow.
- **Hardware / GUI / docker socket** — out of scope; such commands go on the `bypass:` list
  and run on the host.

## Install

Requires the [`lns`](https://get.lns.run) CLI with its service running (`lns service start`).

```sh
make build                 # builds bin/lns-cc
```

Then add this directory as a Claude Code plugin. A `SessionStart` check warns if `lns` is
missing or the service is down.

## Status

Early development. Working today: the PreToolUse hook classifies Tier-A code execution and runs it
in an ephemeral microVM with clean, exit-code-faithful output (verified end-to-end). Still to come:
user configuration (scope, runtime→image map, extra mounts, `env_forward`, bypass), the
`/lns-sandbox` command, and the switch to the persistent backend once
[lensapp/lens-sandbox#94](https://github.com/lensapp/lens-sandbox/issues/94) lands.
