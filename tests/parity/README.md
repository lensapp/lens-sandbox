# Guest network parity harness (phase one)

This harness puts the same network cases to lens-sandbox guests on different
network backends, and writes one machine-readable result per run. Two results
diff against each other, so a backend can be compared with another backend — or
with another build of itself.

It is phase one of the verification programme for the in-process netstack
(issue #380, PR #426).

A backend is **configuration, not a code path**: a name, an `lns` binary, an
`lns-service` binary, and an environment. The harness starts that service on a
private `LNS_HOME` and `LNS_SOCKET_PATH` with `LNS_HEADLESS=1`, runs every case
through it, stops it, and checks that it is gone. You supply the binaries. The
harness records their SHA-256 and `lns --version` in the result. It downloads
nothing.

## Prerequisites

- **macOS on Apple Silicon.** The backends compared here are the macOS ones.
  The harness refuses another host unless you pass `--allow-any-host`.
- **Two signed builds** of `lns` and `lns-service` — one per backend, or one
  build driven by two environments. Check them with
  `tests/parity/scripts/macos-build-check.sh <lns> <lns-service>`.
- **A LAN IPv4 address of this host** (`ipconfig getifaddr en0`). The host
  fixtures bind it, because the netstack refuses the host's loopback and the
  guest's own subnet. A loopback or guest-subnet address is refused before
  anything starts.
- **Ten free TCP ports and one UDP port** from `--base-port` upward (47200 by
  default), plus one loopback port for the witness.
- **The running service must be your own private one.** The harness never
  touches the service in your menu bar: it starts its own on its own socket.

## Run one backend

```
$ cp tests/parity/parity.example.toml parity.toml     # then set `bind`
$ cargo run -p net-parity -- run \
    --config parity.toml --backend netstack --out netstack.json
[service]  pid 54120 home /tmp/parity-54098/home socket /tmp/parity-54098/service.sock
[case]  lease-and-resolver
[pass]  lease-and-resolver
[case]  upload-100m
[pass]  upload-100m
...
netstack: 10 pass, 1 skip, 1 blocked-by-product
written to netstack.json
```

Without a config file, give the backend whole on the command line:

```
$ cargo run -p net-parity -- run \
    --backend vmnet --lns bin/lns --lns-service bin/lns-service \
    --env LNS_NETDEV=vmnet --bind 192.168.1.50 --out vmnet.json
```

`--case NAME` (repeatable) runs a subset. The order is always the registry
order, so `service-stop-with-guests` stays last — it stops the service.

`run` exits 1 when a case failed, 0 otherwise. A skip never fails the run.

## Diff two runs

```
$ cargo run -p net-parity -- diff netstack.json vmnet.json
netstack (lns 0.25.0) vs vmnet (lns 0.25.0)
unexpected differences:
  udp-echo: status pass -> fail
  udp-echo: fixture_empty_datagrams 1 -> 0
expected differences (named in the backend's TOML):
  lease-and-resolver: address 192.168.127.2 -> 192.168.64.3
```

`diff` exits 1 when an unexpected difference remains. A **deliberate** backend
difference — the netstack's own subnet and resolver, for example — is named in
the backend's TOML as `expected_differences`, and lands under the second
heading instead of failing the diff. Timings (`throughput_*`, `duration*`,
`*_ms`) are recorded but never diffed.

## Host fixtures

The runner starts the fixtures in process. `parity fixtures` serves the same
ones on their own, for a manual probe from a guest:

```
$ cargo run -p net-parity -- fixtures --bind 192.168.1.50 --base-port 47200
bidirectional   192.168.1.50:47202
echo            192.168.1.50:47206
half-close-reply 192.168.1.50:47203
host-half-close 192.168.1.50:47204
reset           192.168.1.50:47205
sink            192.168.1.50:47200
source          192.168.1.50:47201
udp-echo        192.168.1.50:47207
witness         127.0.0.1:47208
writing fixtures.json every second; stop with ctrl-c
```

| Fixture | What it does |
|---|---|
| `sink` | Reads to EOF. Counts bytes and hashes the stream. |
| `source` | Sends 100 MiB of the pattern `byte i = (i * 7 + 3) mod 256`, then closes. |
| `bidirectional` | Both at once on one connection. |
| `half-close-reply` | Reads to EOF, then sends a 1 MiB reply. |
| `host-half-close` | Sends 10 MiB, shuts down its write side, keeps reading. |
| `reset` | Sends 5 MiB, then resets the connection (`SO_LINGER` 0). |
| `echo` | Echoes TCP. |
| `udp-echo` | Echoes UDP, **including a zero-length datagram**. |
| `witness` | Binds `127.0.0.1` and records every accept. |

Every fixture writes what it saw — bytes, hashes, EOF, reset, open and close
times — to a JSON file the runner reads.

## The cases

Each case runs one guest and judges it by what the host fixture recorded, not
by the guest's word alone. Images: `docker.io/curlimages/curl:8.11.1` where
curl suffices, `docker.io/library/alpine:3.20` for `dd`, `sha256sum` and
busybox `nc`. Each case verifies the commands it relies on inside the guest
first, and the result records each image's digest.

| Case | What it proves | Pass criteria |
|---|---|---|
| `lease-and-resolver` | The guest gets an address and a resolver. | `ip -4 addr show eth0` gives an address and prefix; `/etc/resolv.conf` names a nameserver. Both recorded. |
| `upload-100m` | A long guest-to-host transfer is byte-exact. | The sink read 104857600 bytes and hashed them to the hash of 100 MiB of zeros. Throughput recorded. |
| `download-100m` | A long host-to-guest transfer is byte-exact. | The guest's `sha256sum` equals the host's hash of the 100 MiB pattern. Throughput recorded. |
| `bidirectional-100m` | Both directions at once stay exact. | Four counts (guest sent, fixture read, fixture sent, guest read) and both hashes agree. |
| `guest-half-close` | A guest half close is carried, not a full close. | The fixture sees EOF after 10 MiB, replies 1 MiB, and the guest reads all of it. |
| `host-half-close` | A host half close is carried. | The guest reads all 10 MiB and keeps sending; the fixture's hash of what the guest sent matches. |
| `reset-mid-transfer` | A reset reaches the guest promptly. | The fixture committed exactly 5 MiB and reset; the guest's command exits non-zero within 5 s. |
| `kill-mid-transfer` | `lns kill` tears the flow down. | The fixture's connection closes within 5 s, no byte arrives after it, and the service's descriptor count returns to its pre-case value within 10 s. |
| `udp-echo` | UDP is carried, empty datagrams included. | The 100-byte datagram comes back as 100 bytes and the empty one comes back empty. PR #426's second review found the empty one dropped: **a fail here is a finding, not a harness bug.** |
| `loopback-witness` | The guest reaches nothing on the host's loopback. | The witness recorded zero accepts. The TOML names each backend's expectation; the case records what happened either way. |
| `create-destroy-20` | Twenty run/kill/remove cycles leak little. | The service's descriptor count is within +3 of the baseline. The delta is always recorded; a larger one is `blocked-by-product` (lens-sandbox issue #427), not a fail. |
| `service-stop-with-guests` | `lns service stop` takes the guests with it. | Both mid-transfer connections close within 10 s and the service PID is gone. |

While the cases run, the harness samples the service PID's RSS
(`ps -o rss=`) and open descriptor count (`lsof -p`, `/proc/<pid>/fd` where
there is no `lsof`) every 5 s into the result.

### The skip contract

A case **skips with a reason** when a prerequisite is missing — it never fails
for the harness's own gaps. It skips when:

- the guest image has no command the case needs (`PARITY_MISSING=<command>`);
- busybox `nc` has neither `-N` nor `-q`, so the guest cannot half-close;
- the image has no tool that sends a zero-length datagram, so the
  empty-datagram half of `udp-echo` cannot be put (a custom image closes this
  in a later phase);
- the host reports no descriptor count for the service.

A case **fails** only when a backend did something measurably wrong, and is
**blocked-by-product** when a known product defect stops it. A run where every
case skipped says so on stderr: it pins nothing.

## The result schema

One `result.json` per backend run, `schema_version = 1`:

```json
{
  "schema_version": 1,
  "harness_revision": "9f1c…",
  "backend": {
    "name": "netstack",
    "env": { "LNS_NETDEV": "netstack" },
    "expectations": { "loopback-witness": "refused" },
    "expected_differences": ["lease-and-resolver"]
  },
  "binaries": [
    { "role": "lns", "path": "bin/lns", "sha256": "…" },
    { "role": "lns-service", "path": "bin/lns-service", "sha256": "…" }
  ],
  "lns_version": "lns 0.25.0",
  "service_pid": 54120,
  "images": [
    { "reference": "docker.io/library/alpine:3.20", "digest": "sha256:…" }
  ],
  "host": { "os": "macos", "os_version": "15.5", "arch": "aarch64", "dns_scope_count": 6 },
  "started_unix_ms": 1757577600000,
  "finished_unix_ms": 1757578320000,
  "cases": [
    {
      "name": "upload-100m",
      "status": "pass",
      "duration_ms": 41230,
      "measures": {
        "sink_bytes_in": 104857600,
        "sink_sha_in": "20492a…",
        "throughput_mib_s": 91.4
      }
    }
  ],
  "samples": [{ "at_ms": 5000, "rss_kib": 120400, "open_fds": 48 }]
}
```

`status` is one of `pass`, `fail`, `skip`, `blocked-by-product`. A case that
failed carries `error`; one that skipped or is blocked carries `reason`.
`measures` holds what was measured on both ends — bytes, hashes, exit codes,
descriptor counts. The diff compares `status` and `measures`.

## Layout and later phases

```
tests/parity/
  src/cases/        one file per group of cases; `cases::all()` is the registry
  src/fixtures/     the host fixtures and their report
  src/result.rs     the result schema
  src/diff.rs       the diff
  scripts/          the macOS build-and-codesign check
```

A later phase adds a case by writing a function in `src/cases/` and registering
it in `cases::all()`. Planned, not in this phase:

- **Policy cases** — an egress policy that allows one destination and refuses
  another, judged at the fixture, and the approval surface that goes with it.
- **Isolation under load** — two guests transferring at once; neither sees the
  other's bytes, and neither starves the other.
- **DNS fixture cases** — an authoritative fixture resolver on the host: TTLs,
  negative answers, truncation and the TCP retry, and a split-horizon scope.
- **A custom guest image** — so the empty-datagram sender, half-close flags and
  raw-socket probes stop depending on what busybox ships.

## CI

CI compiles this crate and runs its unit tests — the result schema, the diff,
and the fixtures in process over loopback — with the rest of the workspace. It
never runs the harness: every case needs a real microVM, which no runner in
this repository can boot. The real run is a person's, on a macOS host.
