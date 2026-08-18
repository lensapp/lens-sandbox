# Lens Sandbox documentation

Lens Sandbox runs AI agents, commands, OCI images, and other untrusted workloads
inside a local microVM. The workload gets a real Linux environment for installs,
builds, and long-running work, but every connection it opens is checked against a
policy you control. Known requests are allowed or denied immediately; anything
unknown pauses for your approval; and your decisions can be saved to a policy file
that future runs reuse.

You drive everything through one binary: the `lns` CLI.

## Start here

- **[Getting started](getting-started.md)** — prerequisites, install, start the
  service, and your first run.

## Guides

- **[Running workloads](running-workloads.md)** — `lns run`, `lns ps`,
  `lns exec`, `lns stop`, `lns start`, `lns rm`: define a sandbox with
  `./lns.yaml`, run it or a published reference, resources, environment
  variables, volumes, published ports, interactive vs. detached sessions,
  and the stopped-run lifecycle.
- **[Example: Claude Code](examples/claude-code/)** — a complete agent recipe:
  manifest, seed config, network allowlist, and credential wiring.
- **[Policy and approvals](policy.md)** — the `lns-local-mixin.yaml` file, being asked
  about what no rule decides, closing a directory, the approval window, and editing
  rules with `lns policy`.
- **[Credentials](credentials.md)** — how placeholders keep real secrets out of
  the workload, and the per-machine value decisions that bind the real value.
- **[Connectors](connectors.md)** — connect workloads to external services
  (credential injection + the routes they need) with `lns connector connect`.
- **[Audit](audit.md)** — the per-run audit chain and verifying it with
  `lns audit`.
- **[The background service](service.md)** — what `lns-service` does and managing
  it with `lns service` and `lns update`.

## Reference

- **[CLI reference](cli-reference.md)** — every command, subcommand, and flag.
- **[Sandbox definition specification](sandbox-spec.md)** — the normative
  `lns.run/v1` document format: every field, every rule, and the three kinds a
  run is assembled from.

### Today and the target

The guides above describe what `lns` does today. The specification describes the
**target** format, which the code does not implement in full yet — the product is
pre-1.0, and each gap closes as a breaking change. Read a guide to use the tool;
read the specification to know what the format is becoming.

## How it fits together

```text
lns (CLI)  ──local Unix socket──▶  lns-service (tray-resident background service)
                                       │
                                       ▼
                                   microVM  (Apple Virtualization, macOS on Apple Silicon)
                                       └─ your workload runs here
```

`lns` is a thin client. The `lns-service` background process owns the microVM
lifecycle, the image and layer caches, the approval window, and the audit writer.
Start it once with `lns service start`; `lns run` talks to it over a local socket.
