# lns-sandbox — Lens Sandbox plugin for Claude Code

Route Claude Code's code-execution commands into [Lens Sandbox](https://lns.run) microVMs.
When the agent runs an untrusted interpreter or a download-and-run one-shot, the command is
intercepted and executed inside an `lns` microVM instead of on the host — with network and
credential access gated by lns policy (you approve egress and secrets via lns's prompts, the
same way a sandboxed `gh` asks for its token).

## How it works

A `PreToolUse` hook on the `Bash` tool inspects each command. If it's code-execution worth
isolating, the hook rewrites it to run inside a per-project microVM:

```
python app.py
   │  PreToolUse hook (lns-cc hook)
   ▼
lns-cc exec python --b64 <base64("python app.py")>
   │  ensures a detached box, then …
   ▼
lns sandbox exec cc-python-<sha8(cwd)> -- bash -lc 'python app.py'
```

The whole command runs in the guest against the project directory (bind-mounted at its real
absolute path), so `cd`, pipes, and compound commands all work. Read-only host inspection
(`ls`, `cat`, `git status`, …) is never intercepted.

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

Early development. P0 (scaffold + passthrough hook + doctor) is in place; classification,
the sandbox exec wrapper, and configuration are landing incrementally.
