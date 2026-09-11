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
  anything starts. Where this host cannot serve a fixture to its own guests, put
  the fixtures on a second machine and name it with `--fixtures-at` instead —
  see [Fixtures on another machine](#fixtures-on-another-machine).
- **Ten free TCP ports and one UDP port** from `--base-port` upward (47200 by
  default), plus one loopback port for the witness, and `--base-port + 20` when
  the fixtures serve their report.
- **A `tcp` rule per fixture destination in the guest's definition.** The harness
  writes this for you, and you have to know why it is there. The sandbox puts its
  own nftables in front of every TCP stream the guest opens, and redirects it to
  the in-guest transparent proxy. The proxy accepts the connect — so `nc -zv`
  reports the port open — and then asks the policy what to do with the stream. An
  `egress.http` catch-all does not decide a raw stream, and a destination no rule
  decides is asked about; headless (`LNS_HEADLESS=1`) there is nobody to ask, so
  the stream is held and no byte reaches the host. Each generated definition
  therefore carries one `egress.tcp` entry per fixture destination:

  ```yaml
  spec:
    image: docker.io/library/alpine:3.20
    egress:
      http:
        - match: "*"
          verdict: allow
      tcp:
        - match: "192.168.1.50:47200"
          verdict: allow
          description: parity host fixture
  ```

  A `tcp` rule must name a port, and no wildcard or range of ports exists, so
  there is one entry per fixture port: the seven TCP fixtures and the UDP echo
  port. The witness is left out on purpose — it binds `127.0.0.1`, and
  `loopback-witness` proves the guest cannot reach it.
- **The running service must be your own private one.** The harness never
  touches the service in your menu bar: it starts its own on its own socket.

## Run one backend

```
$ cp tests/parity/parity.example.toml parity.toml     # then set `bind`
$ cargo run -p net-parity -- run \
    --config parity.toml --backend netstack --out netstack.json
[service]  pid 54120 home /tmp/parity-54098/home socket /tmp/parity-54098/service.sock
[case]  fixture-reachable (budget 120s)
[pass]  fixture-reachable
[case]  lease-and-resolver (budget 120s)
[pass]  lease-and-resolver
[case]  upload-100m (budget 120s)
[pass]  upload-100m
...
netstack: 11 pass, 1 skip, 1 blocked-by-product
written to netstack.json
```

Without a config file, give the backend whole on the command line:

```
$ cargo run -p net-parity -- run \
    --backend vmnet --lns bin/lns --lns-service bin/lns-service \
    --env LNS_NETDEV=vmnet --bind 192.168.1.50 --out vmnet.json
```

`--case NAME` (repeatable) runs a subset. The order is always the registry
order, so `service-stop-with-guests` stays last — it stops the service. A
selection that needs a raw stream to the fixtures always runs `fixture-reachable`
first, whether you named it or not.

Every case declares a budget, and the run prints it. A case past its budget is a
**fail** — `budget exceeded after 121s, 0 bytes seen by the fixture` — and its
guests are killed and removed before the next case starts. The budget covers the
whole case, the guest boot included:

| Case | Budget |
|---|---|
| `upload-100m`, `download-100m`, `bidirectional-100m`, the half closes, `reset-mid-transfer` | 120 s at the 100 MB size |
| `kill-mid-transfer`, `service-stop-with-guests` | 120 s — one cycle group plus one boot |
| `create-destroy-20` | 240 s — 60 s per group of five cycles |
| `fixture-reachable`, `lease-and-resolver`, `udp-echo`, `loopback-witness` | 120 s |

Raise one where your host is slower, either on the command line or in the config:

```
$ cargo run -p net-parity -- run --config parity.toml --backend netstack \
    --budget download-100m=300 --out netstack.json
```

```toml
budgets = { "download-100m" = 300 }
```

The result is written after **every** case, not once at the end, so a run you
stop halfway still leaves a readable `result.json`.

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

The runner starts the fixtures in process. `parity fixtures` runs the same ones
on their own, for a manual probe from a guest — and with `--serve`, for a runner
on another machine:

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

## Fixtures on another machine

Some hosts cannot serve a fixture to their own guests at all. Put the fixtures
on a second machine and read them over HTTP.

**On the fixture machine** (macOS or Linux, on the same LAN), build the harness
from **this same revision** and serve the report:

```
$ cargo build -p net-parity --release
$ ./target/release/parity fixtures --bind 192.168.1.77 --base-port 47200 --serve
bidirectional   192.168.1.77:47202
echo            192.168.1.77:47206
...
report  http://192.168.1.77:47220/report
reset   http://192.168.1.77:47220/reset
health  http://192.168.1.77:47220/health
a runner on another machine reads these with: parity run --fixtures-at 192.168.1.77:47200
writing fixtures.json every second; stop with ctrl-c
```

The report server binds the same address as the fixtures, on
`--base-port + 20`. It serves three routes and nothing else:

| Route | What it does |
|---|---|
| `GET /health` | The harness version and every fixture this process bound. |
| `GET /report` | The current fixtures JSON — the same document `fixtures.json` holds. |
| `POST /reset` | Clears the counters. The runner calls this before every case. |

**On the machine with the guests**, name that address instead of `--bind`:

```
$ cargo run -p net-parity -- run \
    --config parity.toml --backend netstack \
    --fixtures-at 192.168.1.77:47200 --out netstack.json
```

The runner asks `/health` first and refuses to start when nothing answers, or
when the fixtures were built from another harness version — two halves of
different builds measure different things. It then calls `/reset` before every
case and reads `/report` while the case runs. The generated `egress.tcp` rules
and the `fixture-reachable` preflight use the remote address, so the guest is
granted the destinations it will actually meet.

The **loopback witness stays on this machine**. It has to bind the service
host's own `127.0.0.1` for `loopback-witness` to prove anything, so the runner
binds it here and folds its accepts into the report it reads.

The result records which fixtures the run met:

```json
"fixtures": { "host": "192.168.1.77:47220", "mode": "remote", "version": "0.25.0" }
```

`diff` compares two runs only when all three agree. When they do not, it says so
and exits 1:

```
$ cargo run -p net-parity -- diff netstack.json vmnet.json
netstack (lns 0.25.0) vs vmnet (lns 0.25.0)
these runs are not comparable: netstack used in-process fixtures on 192.168.1.49 (harness 0.25.0), vmnet used remote fixtures on 192.168.1.77:47220 (harness 0.25.0)
put both backends to the same fixtures before you read the differences below
no unexpected difference
```

### Why a second machine is sometimes the only way

On a Mac with endpoint-security network extensions — SentinelOne network
monitoring and the Cato Client, for two — a TCP connection to the **host's own**
LAN address or to the bridge address completes its handshake and is then
black-holed. **The symptom is that the handshake succeeds and the accept never
fires.** `nc -zv 192.168.1.49 47200` from a plain shell reports the port open, a
listener bound to that address never gets the accept, and the same listener on
`127.0.0.1` accepts every time. It happens to a plain shell and to a guest
alike, so it is not the sandbox and not a backend.

Loopback is no way out: the netstack refuses `127.0.0.1` by design, which is
what `loopback-witness` pins. So on such a host no fixture is reachable from a
guest, and the fixtures have to live somewhere else. `--fixtures-at` is that
somewhere else.

Run the preflight first to tell this apart from a policy fault. A
`fixture-reachable` that fails against fixtures on a second machine is a real
backend finding; one that fails only against local fixtures, while a plain shell
on the host shows the same black hole, is this.

## The cases

Each case runs one guest and judges it by what the host fixture recorded, not
by the guest's word alone. Images: `docker.io/curlimages/curl:8.11.1` where
curl suffices, `docker.io/library/alpine:3.20` for `dd`, `sha256sum` and
busybox `nc`. Each case verifies the commands it relies on inside the guest
first, and the result records each image's digest.

| Case | What it proves | Pass criteria |
|---|---|---|
| `fixture-reachable` | One raw TCP stream to the host fixtures carries bytes both ways. Runs **first**. | The guest sends 16 bytes to the TCP echo fixture and reads the same 16 back within 10 s. A fail skips every case that needs a raw stream, with `fixture unreachable from the guest: <the preflight error>`, rather than letting each one stall to its own budget. |
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
- the host reports no descriptor count for the service;
- `fixture-reachable` failed, so no raw stream reaches the fixtures and every
  case that needs one is skipped with the preflight's own error.

A case **fails** only when a backend did something measurably wrong, and is
**blocked-by-product** when a known product defect stops it. A run where every
case skipped says so on stderr: it pins nothing.

## The result schema

One `result.json` per backend run, `schema_version = 2`:

```json
{
  "schema_version": 2,
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
  "fixtures": { "host": "192.168.1.50", "mode": "in-process", "version": "0.25.0" },
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

Every case records what the host fixtures saw while it ran —
`fixture_seen_connections`, `fixture_seen_bytes_in`, `fixture_seen_bytes_out` —
whatever its verdict. A stall is therefore readable from `result.json` alone: a
case that failed on its budget with `fixture_seen_bytes_in: 0` never moved a
byte, and one with a partial count stopped partway.

`fixtures` names the machine the cases were put to, whether it was this process
(`in-process`) or a second machine (`remote`), and the harness version that
served them. Two results diff only when all three agree.

`status` is one of `pass`, `fail`, `skip`, `blocked-by-product`. A case that
failed carries `error`; one that skipped or is blocked carries `reason`.
`measures` holds what was measured on both ends — bytes, hashes, exit codes,
descriptor counts. The diff compares `status` and `measures`.

## Layout and later phases

```
tests/parity/
  src/cases/        one file per group of cases; `cases::all()` is the registry
  src/fixtures/     the host fixtures and their report
                    report_server.rs serves it over HTTP; remote.rs reads it
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
