# Audit

Every run records an audit log: an integrity-checked record of what happened —
egress attempts, policy decisions, file activity, and outcomes. The log stays on
your machine.

## The audit chain

Audit events are written as newline-delimited JSON (`audit.jsonl`), one event per
line. Each event is hash-chained to the one before it: every line carries the hash
of the previous line, and the first line chains from a genesis hash of all zeros.
Editing or reordering any line breaks the chain from that point on, so in-place
tampering is detectable on the next `lns audit`.

The chain alone cannot catch a log that has simply had its tail cut off (or been
rolled back to an earlier, still-valid prefix) — a shorter chain is still
internally consistent. To close that gap, the background service keeps a separate,
service-owned anchor file (`audit.anchor`, created `0600` next to `audit.jsonl`)
that records the latest head hash and event count and is fsync'd on every append.
`lns audit` compares the log against the anchor as it reads: a log that is shorter
than, or diverges from, the anchored head and count is flagged as truncated.

This makes the log **integrity evidence, not authenticity**: it detects accidental
or after-the-fact corruption, truncation, and rollback. It is not a cryptographic
proof of authorship. An attacker running as the same user can rewrite both the log
and the anchor together, so a same-uid attacker who holds the anchor file is out of
scope here. Binding the log to a key the workload can't reach (an HMAC chain, or an
anchor mirrored to external append-only storage) is tracked as follow-up.

## Reading the log checks integrity

`lns audit` shows one chronological timeline of every event across all sandboxes —
egress and mounts from each run's log, plus the approvals, sign-ins, and credential
uses recorded across runs in the durable connection ledger. `lns audit <sandbox>`
scopes it to one run (by run id or unique id prefix). See the
[CLI reference](cli-reference.md) for filters.

Provisioning a run's [declared tools](running-workloads.md#tools--declared-toolchains)
is recorded in that run's chain: what was fetched, from where, and the exact
version it resolved to. Warm runs reuse the machine cache and fetch nothing, so
they add no provisioning events. `lns pull` provisions a published sandbox's
pinned tools ahead of its first run, before any run exists — those fetches are
recorded on the same durable chain as approvals and sign-ins, with the pull in
place of a sandbox name, so nothing is acquired without a record.

Integrity is verified **as the log is read** — there is no separate verify step. As
`lns audit` reads each chain it compares it against its anchor, and if anything is
wrong it prints an inline `audit integrity:` warning and still lists what's there,
so a compromised log surfaces even when you only meant to look:

- an edited or reordered line — the chain breaks from that point on:

  ```text
  audit integrity: chain broken at line 42 (<reason>) — entries shown may have been altered
  ```

- a log shorter than, or diverged from, its anchor (a truncated tail, or a rollback
  to an earlier prefix):

  ```text
  audit integrity: log truncated or rolled back (<reason>) — entries may be missing
  ```

- a log that holds events but has no anchor beside it, so truncation and rollback
  can't be checked at all:

  ```text
  audit integrity: no anchor beside the log — truncation or rollback cannot be detected
  ```

  (An empty log is not flagged — it has nothing to protect, and a log that once held
  events but was wiped is caught as a truncation against its surviving anchor.)

- a single line that can't be parsed as an audit event — that one entry is skipped and
  flagged, and the rest of the timeline still lists:

  ```text
  audit integrity: unreadable entry at line 12 of <path> (<reason>) — that entry is not shown
  ```

The warning marks the entries untrustworthy; the events are still printed so you can
see what the log claims.

## Where logs live

Audit logs and their anchors are kept per run under the service's data directory, so
the trail outlives the ephemeral run overlay under the cache directory:

```text
~/.lns/runs/<run-id>/audit.jsonl
~/.lns/runs/<run-id>/audit.anchor
```

The connection ledger sits alongside them at `~/.lns/ledger.jsonl`. `LNS_HOME`
moves the whole directory, the audit trail with it.

## See also

- [Policy and approvals](policy.md) — the decisions that audit records.
- [CLI reference](cli-reference.md) — `lns audit`.
