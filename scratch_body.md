## Why

`lns audit <id>` only answered *"is the hash chain intact?"* — nothing about **what** you let through. There was no way to ask "did I connect to GitHub, with what scope? which key went into which sandbox, and when?"

This turns `lns audit` into a single timeline over a durable **connection ledger** plus the per-run logs, so that history survives ephemeral run cleanup. Two structural problems surfaced while building it and are fixed here too: run ids were reusable (**opaque run id**), and the command had grown four subcommands (**one unified command**).

## What — one `lns audit`

`lns audit` is now a single, subcommand-free command. Bare, it shows one chronological (newest-first) timeline of **every** event across **all** sandboxes; a positional scopes it to one.

```bash
lns audit                                    # every event, every sandbox, newest first
lns audit <sandbox>                          # scope to one: run id or unique id prefix
lns audit [--integration <id>] [--kind <k>] [--json]
```

```
WHEN                 RUN           KIND        DETAIL
2026-06-29 15:12:00  9e8d7c6b0000  credential  use some-provider fp 9c2f1a3d → api.some-provider.example
2026-06-29 14:00:00  1a2b3c4d0000  connection  connect some-oauth (oauth) @hchen [repo, read:org]
2026-06-29 13:32:00  1a2b3c4d0000  approval    network allow-always api.example.test:443  [policy-ambiguous]
2026-06-29 13:30:00  1a2b3c4d0000  egress      GET api.example.test:443 — allowed once → 200 success
2026-06-29 13:05:00  1a2b3c4d0000  volume      data → /data
2026-06-29 13:00:01  1a2b3c4d0000  env         injected: GITHUB_TOKEN, PATH
```

It merges two sources by timestamp: the per-run audit logs (`egress` / `env` / `volume` / `bind`) and the durable connection ledger (`approval` / `connection` / `credential`). Filters compose: `--integration <id>` (ids from `lns integration list`, also shown in `DETAIL`), `--kind` (`egress`/`env`/`volume`/`bind`/`approval`/`connection`/`credential`), `--json` (one raw event per line). Integrity is verified **inline** as the log is read — a tampered/truncated/anchorless chain prints an `audit integrity:` warning and still lists what's there.

**Removed (breaking):** the `show` / `log` / `connections` / `verify` subcommands, the by-integration rollup, and the explicit verify verb (verification is internal now — no exit-code check, no `--allow-missing-anchor`).

## Run identity — opaque ids (breaking)

Runs were identified by a sequential `u32` seeded at boot from the highest-numbered dir under the **ephemeral cache** `runs/`. The new ledger is **durable** (`data_root`) and references runs by that number — so once the cache is pruned (the ledger is designed to outlive run dirs), the counter reset and new runs reused ids, conflating unrelated runs.

Runs now get an **opaque 32-char hex id** (Docker-style) from 16 random bytes — no counter, no ephemeral seeding — so ids never repeat and never collide across machines. The auto-name (`calm-finch`) stays the friendly handle; ids resolve by exact value, name, or unique prefix and render as a 12-char short id. Unknown-run verbs report a uniform `no such run: <id>`.

**Breaking:** run ids are opaque strings, not integers; no migration of old numeric run dirs or ledger records.

## Design

- **Durable global ledger** under `data_root` (not `cache_root`), so history outlives ephemeral run dirs. Same `AuditChain` hash-chaining.
- **Typed `LedgerEvent` schema** in `lns-ipc` (`approval` / `connection` / `credential_use`), shared by reader and writer. Every record is run-attributed (`run` + `microvm`); there are no runless events — so the timeline slices cleanly by sandbox.
- **Secrets never recorded**: API keys identified by a sha256 **fingerprint** only; OAuth records scopes + account + expiry.
- **Recorder seam**: sessions hold an optional `LedgerRecorder` via `OnceLock`; a write failure is logged, never propagated, so recording can't break the run.
- **Single-writer ledger**: every concurrent run's append serializes under a process-global write lock, off the run's async worker, so the shared hash chain can't interleave.
- **Opaque run id**: random hex, resolved by id / name / unique prefix; 12-char short id for display.
- **Unified read**: `lns audit` merges the per-run logs and the ledger into one newest-first timeline; integrity is checked inline, no verb.

## Status — complete, all green

- [x] `lns-ipc`: typed schema + `data_root` paths + `fingerprint()` + opaque-id types + `short_run_id`
- [x] `lns-service`: ledger writer + `RunLedgerRecorder` seam; opaque-id allocation/resolution; `approval` / `connection` (scopes + account + expiry) / `credential_use` (fingerprint) recording wired onto both sessions
- [x] `lns-cli`: single `lns audit` timeline (scope + `--integration`/`--kind`/`--json`), inline integrity warning
- [x] run id migrated `u32` → opaque hex across IPC / service / CLI / tests
- [x] docs (`audit.md`, `cli-reference.md`) + Gherkin (Layer 2 + e2e) rewritten to the unified surface

Every new/changed file at 100% coverage; lint, complexity, coverage, test, and e2e all green locally.

## Post-review hardening

Self-review and iteration surfaced several issues; all fixed and covered.

**Correctness**
1. **Concurrent-run chain corruption** — appends now serialize under a process-global write lock (regression-pinned by a concurrent-append test).
2. **Blocking fsync on the async executor** — `record()` offloads to a dedicated FIFO writer thread, drained on drop.
3. **Run-id reuse vs. the durable ledger** — fixed by the opaque-id change above.

**Closing the audit gaps**
4. **Credential & integration approval decisions were never recorded** (incl. every deny) — `CredentialSession` now emits `Approval{kind:Credential}` on every allow/deny and the offer path emits `Approval{kind:Integration}` on connect success/failure.
5. **OAuth account is resolved** from the integration's userinfo endpoint (catalog `userinfoEndpoint` + `accountField`, behind a `UserInfoFetcher` port); best-effort.
6. **Granted scopes** recorded from the token response; a refresh preserves the scopes/account resolved at sign-in.

**Tracked follow-up (not in this PR)**
- The ledger records a credential/connection **decision per run**, not every per-run injection of an already-armed credential, so the timeline under-reports runs that reuse a credential without re-prompting. Accurate per-request destinations need a guest→host injection-audit hop (platform-coupled). Both out of scope here.
- `lns audit <sandbox>` scopes by id / unique prefix only: the timeline is file-based (per-run dirs + ledger) and the friendly name is never persisted per-run, so name-scoping would silently miss a run with no ledger activity. Scoping by name would need the name written into the run dir; deferred rather than promised.


