# Getting started

This guide takes you from nothing installed to a workload running inside a
sandbox.

## Prerequisites

Lens Sandbox boots a real microVM under a hardware hypervisor:

- **macOS on Apple Silicon** (M-series) — uses Apple's Virtualization framework,
  which ships with macOS, so there's nothing extra to install. Intel Macs cannot
  host the guest VM and are rejected by the installer.
- **Linux on x86_64 or aarch64** — uses KVM via Cloud Hypervisor. You need
  `/dev/kvm` accessible to your user (typically `sudo usermod -aG kvm $USER`) and
  the `cloud-hypervisor` and `virtiofsd` binaries available on `PATH` (or pointed
  to with `LNS_CLOUD_HYPERVISOR_BIN` / `LNS_VIRTIOFSD_BIN`). The installer checks
  for both and tells you what's missing.
- **Windows 11 with WSL2** — Run Lens Sandbox inside WSL2. It uses the Linux
  build, with the same requirements. To run workloads, turn on nested
  virtualization so WSL2 can start virtual machines. Windows 11 turns this on by
  default.

The security model is identical everywhere: the same per-directory policy, the same
credential-shaped placeholders, the same "policy you run into, not write."

## Platform support

macOS on Apple Silicon, Linux (x86_64 / aarch64), and Windows 11 with WSL2 are
supported. On Windows, Lens Sandbox runs inside WSL2 as the Linux build. Turn on
nested virtualization to run workloads. Running Windows directly, without WSL2,
is not supported.

The Linux binaries require **glibc 2.35 or newer** — Ubuntu 22.04 LTS and newer,
Debian 12 and newer, Fedora 36 and newer, and current rolling releases (Arch,
openSUSE Tumbleweed) all qualify. Check yours with `ldd --version`.

## Install

```bash
curl -fsSL https://get.lns.run | bash
```

This installs two binaries — the `lns` CLI and the `lns-service` background
service — into `~/.local/bin` by default. Set `INSTALL_DIR` to choose another
location:

```bash
curl -fsSL https://get.lns.run | INSTALL_DIR=/usr/local/bin bash
```

Make sure the install directory is on your `PATH`, then confirm:

```bash
lns --version
```

## Start the background service

`lns run` talks to the `lns-service` background process over a local socket. Start
it once per login session:

```bash
lns service start
```

This launches the tray-resident service (you'll see its menu-bar / system-tray
icon) and waits until it's ready. Check on it any time:

```bash
lns service status
```

The service keeps running across sandbox runs — it owns the microVM lifecycle, the
cache, the approval window, and the audit log. Stop it from the tray's **Quit**
menu or with `lns service stop`. See [the background service](service.md) for
details.

## Your first run

Change into your project directory, scaffold its sandbox definition, then run it:

```bash
cd ~/dev/my-app
lns init
lns run
```

Before the workload starts, `lns` prints a summary of what it's about to do and
which policy applies:

```text
lns run
  Image:     alpine:3.20 (resolving…)
  Resources: 1 vCPU · 512 MiB
  Flags:     -i -t
  Ports:     (none)
  Policy:
    file: /Users/you/dev/my-app/lns-policy.yaml
    default verdict: ask
    rules: none defined; anything else asks
    source: auto-created (no policy in this directory)
```

You run Lens Sandbox from a project directory — that's where it looks for
`lns-policy.yaml`, creating one with a default verdict of `ask` the first time. To
give the workload your actual project files, bind-mount the directory with
`-v "$(pwd)":/work` (see [Running workloads](running-workloads.md)); for a
portable definition use a declarative bind with `source: .`; for scratch
space that persists across runs instead, attach a named volume. The workload's
working directory comes from `spec.workdir` when declared, otherwise from the image.

## Define a sandbox

The sandbox you keep is its **definition** — a `./lns.yaml` file that pins the base
image plus its command, environment, policy, and connectors. One directory is
one sandbox. Scaffold it with `lns init`:

```bash
lns init
```

That writes a starter `./lns.yaml`:

```yaml
apiVersion: lns.run/v1
kind: Sandbox
metadata:
  name: sandbox
spec:
  image: docker.io/library/alpine:3.20
  command: sh
  workdir: /workspace
  env: {}
  resources:
    cpu: 1
    memory: 512Mi
  policy:
    defaultVerdict: ask
    allowedRoutes: []
  connectors: []
  credentials: []
  volumes:
    - type: bind
      source: .
      target: /workspace
  filesets: []
  ports: []
```

Every `spec` field is present with its default so editing is filling in a blank,
not learning a schema. Edit `spec.image` (and the fields you need), then check it
offline —
`validate` runs schema, cross-field, and secret checks without touching the
network or the service, and a target-less `lns inspect` renders the effective
definition:

```bash
lns sandbox validate     # -> lns.yaml is valid.
lns inspect              # -> the merged, resolved definition (offline)
```

Run it by omitting the reference — `lns run` with no argument runs the
`./lns.yaml` in the current directory:

```bash
lns run
```

When you're ready to share the sandbox, publish it in one step. `lns push` builds
`./lns.yaml` and uploads it to a registry as a sandbox artifact; anyone can then
`lns pull` or `lns run` that reference:

```bash
lns push ghcr.io/acme/reviewer:1.0.0
```

## What happens on an unknown request

With the default `ask` verdict, the workload runs normally until it tries to reach
the network. When it opens a connection that no rule covers, the request pauses and
an approval window appears from the background service showing the host and the
action (for example `CONNECT api.github.com:443`). You choose:

- **Allow once** / **Deny once** — applies to this request only.
- **Allow always** / **Deny always** — also writes a matching rule to
  `lns-policy.yaml`, so the same question isn't asked again.

A denied request fails at the boundary the way a real network failure would — a
refused connection or a failed DNS lookup — never a silent success.

If no one answers, the request times out and is denied.

## Where to go next

- Author, run, distribute, and manage a sandbox; tune resources, mount volumes,
  publish ports, and run commands directly:
  [Running workloads](running-workloads.md).
- Pre-author rules instead of approving them interactively:
  [Policy and approvals](policy.md).
- Give the workload credential-shaped placeholders while real secrets stay
  outside it: [Credentials](credentials.md).
