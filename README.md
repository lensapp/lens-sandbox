# LNS

[![CI](https://github.com/lensapp/lens-sandbox/actions/workflows/ci.yml/badge.svg)](https://github.com/lensapp/lens-sandbox/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust 1.97+](https://img.shields.io/badge/rust-1.97%2B-orange.svg)](rust-toolchain.toml)

> **The sandbox you'll actually leave running.**
>
> Run AI agents, commands, and OCI images locally. Control access into and out of the sandbox.

LNS is a local desktop app that runs AI agents, commands, OCI images, and other untrusted workloads inside a microVM on your own machine. The workload gets a real environment for installs, tools, builds, and long-running work — but inbound and outbound access is checked against policy. Allowed requests proceed; denied requests are blocked at the boundary; and when no rule matches yet, LNS starts an approval flow and remembers your decision.

## Install

```bash
curl -fsSL https://get.lns.run | bash
```

This installs the `lns` CLI and the `lns-service` background service. LNS runs on **macOS on Apple Silicon** (M-series, via the Virtualization framework) and **Linux on x86_64/aarch64** (glibc 2.35+, e.g. Ubuntu 22.04+; via KVM + Cloud Hypervisor — needs `/dev/kvm` plus `cloud-hypervisor` and `virtiofsd`). On **Windows 11**, run it inside WSL2, which uses the same Linux build — turn on nested virtualization to run workloads.

## Quickstart

```bash
lns service start              # start the background service once per session
cd ~/dev/my-app
lns run nousresearch/hermes-agent \
  -v hermes:/opt/data \
  -p 8642:8642 -p 9119:9119 \
  -- hermes
```

`lns run` prints a summary of the run and the policy that applies before the workload starts:

```text
lns run
  Image:     nousresearch/hermes-agent (resolving…)
  Volume:    hermes → /opt/data
  Resources: 1 vCPU · 512 MiB
  Flags:     -i -t
  Ports:     127.0.0.1:8642 -> 8642, 127.0.0.1:9119 -> 9119
  Policy:
    file: /Users/you/dev/my-app/lns-local-mixin.yaml
    default verdict: ask
    rules: none defined; anything else asks
    source: auto-created (no policy in this directory)
```

When the workload opens a connection no rule covers, the background service raises an approval prompt showing the host and action (e.g. `CONNECT api.linear.app:443`). You choose allow/deny — once, or always. "Always" writes a matching rule to `lns-local-mixin.yaml`, so future runs load the decision automatically.

## What it does

- **Isolates where the workload runs.** A local microVM instead of the host machine, so the host stays outside the blast radius.
- **Controls access in and out.** Inbound ports, outbound network and API calls, package downloads, container calls, and credential-backed actions are checked against policy.
- **Approves what policy doesn't know yet.** No matching rule triggers an approval flow; decisions are remembered in a policy file.
- **Keeps real secrets outside the workload.** Tools receive credential-shaped placeholders; real tokens are exchanged at the boundary only when policy allows.
- **Records a local audit history.** Every run produces a tamper-evident record you can verify with `lns audit`.

## Documentation

Start with [`docs/getting-started.md`](docs/getting-started.md), or browse the [documentation index](docs/README.md): [running workloads](docs/running-workloads.md), [policy & approvals](docs/policy.md), [audit](docs/audit.md), [the background service](docs/service.md), and the [CLI reference](docs/cli-reference.md).

## Repository

```text
crates/lns-cli/             The lns developer CLI — the shipping artifact
crates/lns-service/         Tray-resident background service (microVM lifecycle, caches, audit)
crates/lns-ipc/             Shared CLI ↔ service IPC contract
crates/lns-init/            Static-musl PID 1 for the guest microVM
crates/lns-session/         Host ↔ guest session wire protocol
crates/lns-session-broker/  Guest-side session host
crates/bump-kernel/         Operator tooling for the kernel pin
scripts/lns-install/        Installer published to get.lns.run
docs/                       User documentation
```

## Development

```bash
make dev                                   # fast debug build (inner loop)
make lint && make complexity && make coverage   # the pre-push verification gate
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) to get started and [`AGENTS.md`](AGENTS.md) for the full architecture, test pyramid, and project conventions.

## Open Source

- [CONTRIBUTING.md](CONTRIBUTING.md) — development workflow and contribution guidance
- [SECURITY.md](SECURITY.md) — vulnerability reporting and security scope
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) — community expectations

## License

Apache 2.0 — see [LICENSE](LICENSE).
