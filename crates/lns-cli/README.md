# LNS

_This README describes where LNS is going._

> **The sandbox you'll actually leave running.**
>
> Run AI agents, commands, and OCI images locally. Control access into and out of the sandbox.

LNS is a local desktop app for running AI agents, commands, OCI images, scripts, generated code, and build jobs inside a local microVM. Inbound ports and services, outbound network calls, API calls, package downloads, container and service calls, and credential-backed actions are checked against policy. When there's no rule yet, LNS starts an approval flow. Decisions can be exported to a policy file and loaded by future sandbox runs.

```bash
$ cd ~/dev/my-app
$ lns run ghcr.io/anthropic/claude-code

  ✓ pulled ghcr.io/anthropic/claude-code
  ✓ booted sandbox in ~/dev/my-app   (2 cpu · 2 GiB · 4.1s)
  > _
```

A few seconds later the workload tries something with no matching rule. LNS asks:

```
┌─ lns ─────────────────────────────────────────────────────┐
│  claude-code wants to:                                    │
│    POST https://api.linear.app/graphql                    │
│                                                           │
│  From: ~/dev/my-app                                       │
│  Using: linear credential                                 │
│                                                           │
│  [ allow once ]  [ always allow ]  [ deny ]               │
└───────────────────────────────────────────────────────────┘
```

The workload retries and continues. Your decision is remembered for this directory — next time anything reaches Linear here, no prompt.

LNS runs _any_ workload that needs a sandbox boundary: an AI coding agent, a CI script, a Python interpreter, a dev container, that sketchy `npm install` you'd rather not run on your laptop:

```bash
lns run ghcr.io/anthropic/claude-code     # an AI agent
lns run python:3.12 -- python build.py    # an interpreter
lns run ghcr.io/myorg/scraper:dev         # your own image
lns run node:20 -- npm test               # a one-shot CI step
```

Same boundary, same policy UX, same local audit history. No TUI to learn, no projects to manage. Decisions are stored in the directory you run from.

---

## Install

```bash
curl -fsSL https://get.lns.run | bash
```

One line on macOS (Apple Silicon) and Linux (arm64, amd64). The installer verifies a SHA-256 checksum, drops a binary in `~/.local/bin`, and prints the one line to add to your shell rc so `lns` is on `$PATH`.

```bash
lns update     # update in place
lns doctor     # verify install, runtime, config
```

---

## 60-second quickstart

```bash
cd ~/dev/your-repo
lns run ghcr.io/anthropic/claude-code
```

The workload boots, you start working. When it tries something with no matching rule, LNS asks `allow once / always allow / deny`. Click. The workload retries and proceeds.

Every "always allow" answer is remembered for this directory and applies forever after. The first run is the noisiest. By the third, the sandbox is mostly silent — and the policy is real.

That's it. You can stop reading here; the rest is reference.

---

## The mental model

Three things to know:

```
image       a container image you run (ghcr.io/anthropic/claude-code, python:3.12, your own)
directory   where decisions are stored — every decision is scoped to one
decision    allow / deny / credential — remembered for this directory
```

Five verbs to know:

```
run         start a workload in a sandbox
attach      reconnect to a running sandbox
logs        watch what it's doing
audit       watch what it's reaching for
stop        end the session
```

If you remember those, you know LNS. Everything else is progressive disclosure.

---

## How decisions work

Every outbound call, inbound port, file access, and credential lookup goes through the sandbox boundary. When the workload tries something not in your policy, three things happen:

1. **The workload gets a real error.** `ECONNREFUSED`, `permission denied`, `gh: not signed in`, a `403`, a failed DNS lookup — whatever the underlying call would have produced. Not a silent fallback.
2. **You get prompted** with the actual request, the error the workload saw, and three buttons: `allow once`, `always allow`, `deny`.
3. **Your answer is recorded.** `allow once` covers this attempt; `always allow` is remembered for this directory so it never asks again.

```
┌─ lns ─────────────────────────────────────────────────────┐
│  claude-code wants to:                                    │
│    POST https://api.linear.app/graphql                    │
│                                                           │
│  From: ~/dev/your-repo                                    │
│  Triggered by: curl -X POST api.linear.app/graphql ...    │
│  Workload saw: blocked by network policy                  │
│                                                           │
│  [ allow once ]  [ always allow ]  [ deny ]               │
└───────────────────────────────────────────────────────────┘
```

### Credential prompts look the same

Real secrets stay outside the workload. When something inside the sandbox runs `gh auth status` and gets "not signed in", you see:

```
┌─ lns ─────────────────────────────────────────────────────┐
│  claude-code wants a credential:                          │
│    github                                                 │
│                                                           │
│  [ provide ]  [ skip ]                                    │
└───────────────────────────────────────────────────────────┘
```

The workload receives a credential-shaped placeholder so software behaves normally. Real tokens, keys, and credential material stay outside the workload. LNS exchanges placeholders for real credentials at the boundary when policy allows the request.

---

## Policy files

Decisions persist beyond the run. A policy file holds rules. Users can write rules ahead of time when they know them. Approvals from a running sandbox can be exported to a policy file. A future sandbox run can load the file at boot so the same approvals are already in place.

The file can be shared — committed to a repo, passed to a teammate, attached to a runbook. Teams can standardize local sandbox behavior by sharing policy files.

Scope is hierarchical:

| Scope | What it's for | Travels with |
|-------|---------------|--------------|
| **IT-managed** | Constraints your organization or IT enforces on this machine | Your machine (set by your IT) |
| **User defaults** | Decisions and preferences you want everywhere you use LNS | Your machine |
| **Team baseline** | Allow/deny/credential references shared by everyone working on it | The repo |
| **Personal directory** | Your "always allow" clicks for this specific directory | Your machine, this directory |

User defaults, team baseline, and personal directory compose with later (more-specific) layers winning. **IT-managed rules form a floor: user actions can always tighten, but cannot loosen an IT-set deny.** Storage shape — filenames, format, on-disk layout — is open.

---

## Daily workflows

### Run a workload

```bash
lns run ghcr.io/anthropic/claude-code           # any OCI image reference
lns run python:3.12 -- python a.py              # short reference (default registry)
lns run ghcr.io/myorg/tool:dev --name reviewer  # custom sandbox name
```

### List, attach, stop

```bash
lns ps
   ID            NAME         IMAGE                          STATE    CREATED              STARTED              CPU %  MEM
   1a2b3c4d0000  build        python:3.12                    running  2026-08-30 09:12:00  2026-08-30 09:12:00  12.5%  184.0 MiB / 1.0 GiB
   5e6f7a8b0000  claude-code  ghcr.io/anthropic/claude-code  running  2026-08-28 14:03:00  2026-08-30 08:41:00  3.1%   512.0 MiB / 2.0 GiB

lns attach claude-code
lns stop build
```

### Tail logs and audit

```bash
lns logs claude-code --follow
lns audit claude-code --follow --format jsonl | jq '.event'
lns audit claude-code --denied        # just the blocks
```

```
14:02:17  ✓ github.com           git push          → 200
14:02:21  ✓ api.openai.com       chat completions  → 200  (model=gpt-4o-mini)
14:02:25  ✗ pastebin.com         DNS               ← blocked by policy
14:02:25  ↳ workload received ECONNREFUSED.
```

---

## How the microVM works

LNS embeds a microVM driver (`Vz`, `KVM`, `firecracker` — depending on platform). There is no Docker, no Apple Container CLI, no Lima, no other dependency. The binary you installed is the entire runtime.

Each `lns run` boots a fresh, ephemeral microVM, fetches the OCI image, runs it, and tears the microVM down on exit. Container tooling can run inside the microVM when needed, so the workload can do real development work — local containers, services, build tools, test runners, databases, package managers — without touching the host container runtime. CPU, memory, and disk are configurable per run.

---

## Behind the CLI

When you run a `lns` command, the CLI is a thin client that talks to a user-level LNS process. That process does the real work — supervises microVMs, enforces policy, coordinates state across runs so `lns attach`, `lns status`, and the prompt surface see the same thing. It starts on first use, runs as you (not root, never system-wide), and exits after an idle timeout when nothing's left. No Docker daemon, no always-on service.

---

## When things go wrong

```bash
lns doctor
   ✓ binary up to date
   ✓ virtualization backend ok
   ✓ ~/.lns writable
   ✓ network policy backend ok
   ✗ ghcr.io login expired — fix: lns auth login
```

Every error message has a `fix:` line. Every `fix:` is a single command.

---

## Configuration

```bash
lns config get                       # effective config
lns config set microvm.memory 4GiB   # default microVM size
lns config edit                      # open in $EDITOR
```

Empty by default.

---

## Relationship to Lens Agents

LNS is the local product. It runs on a developer's machine. Lens Agents is the centrally managed platform for IT teams that need managed hosting, central administration, central audit, and governed execution across many sandboxes. LNS is not a feature of Lens Agents — both adopt the same policy model, so the same policy can span from one developer's decision to organizational governance without a migration step.

---

## Philosophy

1. **The secure path is the easy path.** A sandbox you turn off protects nothing. LNS is built so you don't turn it off.
2. **Asks at the right moment, not before.** No upfront policy archaeology. You don't have to predict what the workload will need.
3. **Remembers what you decided.** The first run is the noisiest. By the third, the sandbox is mostly silent — and the policy is real.
4. **Ephemeral by default.** Each run boots a fresh microVM and tears it down on exit. State persists only when you ask for it.
5. **No external dependencies, no privileged service.** The binary you installed is the entire sandboxing runtime.
6. **Real secrets stay outside the workload.** Credential-shaped placeholders enter; real keys never do.
7. **The directory is the unit.** No project graph, no policy registry. The folder you're in is where decisions stick.

---

## Help

```bash
lns help
lns help run
lns feedback        # opens an issue with diagnostics attached
```

From the team behind Lens Kubernetes IDE — used by over 1 million developers.

---

## Development

From the workspace root:

```bash
make build-lns       # build bin/lns (release + macOS codesign)
make lint            # cargo fmt --all -- --check + cargo clippy --workspace --all-targets -- -D warnings
make complexity      # per-crate cognitive complexity gate
make test            # cargo test --workspace --exclude e2e-tests --all-targets
make coverage        # instrumented tests + per-file 100% coverage floor
```

For crate-scoped iteration: `cd crates/lns-cli && cargo test` (or `cargo test -p lns-cli` from anywhere).

The local pre-push gate is `make lint && make complexity && make coverage` — see [`CLAUDE.md`](../../CLAUDE.md) for the full definition and the broader CI required suite.

Specs live in `specs/**/*.feature` (cucumber-rs). Nothing ships without one.
