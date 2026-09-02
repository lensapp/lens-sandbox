# Security Policy

LNS is security-sensitive software. Its job is to run untrusted workloads — AI agents, commands, OCI images, generated code — inside a microVM while controlling access into and out of the sandbox. The host/guest isolation boundary, the policy-enforcement path, credential handling, and the audit chain are all security boundaries.

## Reporting Vulnerabilities

Please do not open a public issue for suspected vulnerabilities.

Report security issues by emailing:

```text
security@lenshq.io
```

Include as much detail as possible:

- Affected component or file path, if known
- Impact and the security boundary you believe is crossed
- Reproduction steps or proof of concept
- Environment details: OS, hardware, microVM runtime (Vz / KVM), kernel version, and the workload (image or command) involved
- Whether the issue involves sandbox escape, network policy bypass, credential exposure, or audit-chain tampering

We will acknowledge reports as quickly as practical and coordinate remediation before public disclosure.

## Security Scope

Security-sensitive areas include:

- Guest/host isolation — the microVM boundary (Vz on macOS, KVM on Linux) and the host-side service that owns its lifecycle
- Policy enforcement — whether a workload can reach a network gate, port, or credential-backed action that policy did not approve
- The approval flow — decisions written to the run's own `decisions.yaml`, and whether a denied request can be silently turned into an allowed one
- Credential handling — placeholder substitution and boundary credential exchange, and whether any real secret can reach the guest VM
- The audit chain — tamper-evidence of the recorded run history, and the `lns audit` verifier
- The host IPC surface — the local Unix socket between `lns` and `lns-service`
- OCI ingest and the content / layer caches

## Security Boundaries

LNS runs untrusted workloads on a developer's machine. The intended boundary is: the workload runs inside the microVM with controlled inbound and outbound access; the host stays outside the blast radius; real secrets stay outside the workload; and every run produces a tamper-evident audit record.

When in doubt, treat any change touching the microVM boundary, policy enforcement, credential exchange, the audit chain, or the host IPC surface as security-sensitive.
