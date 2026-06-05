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
`lns audit` compares the log against the anchor: a log that is shorter than, or
diverges from, the anchored head and count is reported as truncated, not verified.

This makes the log **integrity evidence, not authenticity**: it detects accidental
or after-the-fact corruption, truncation, and rollback. It is not a cryptographic
proof of authorship. An attacker running as the same user can rewrite both the log
and the anchor together, so a same-uid attacker who holds the anchor file is out of
scope here. Binding the log to a key the workload can't reach (an HMAC chain, or an
anchor mirrored to external append-only storage) is tracked as follow-up.

## Verifying a run

Use the run id that `lns run` prints (`✓ started run #<id>`, also shown by
`lns ls`):

```bash
lns audit 7
```

On an intact chain it reports the number of events and exits `0`:

```text
Verified 128 audit events
```

If a line has been edited or reordered, it reports the first broken line and exits
non-zero:

```text
audit chain TAMPERED at line 42: <reason>
```

If the log is shorter than, or diverges from, the anchor (a truncated tail or a
rollback to an earlier prefix), it reports the mismatch and exits non-zero:

```text
audit chain TRUNCATED: <reason>
```

An empty log verifies successfully (zero events) only when no anchor records a
longer history. A missing log is reported distinctly from an empty one.

## Where logs live

Audit logs and their anchors are kept per run under the service's cache directory:

```text
<cache>/lns/runs/<run-id>/audit.jsonl
<cache>/lns/runs/<run-id>/audit.anchor
```

On macOS `<cache>` is `~/Library/Caches`.

## See also

- [Policy and approvals](policy.md) — the decisions that audit records.
- [CLI reference](cli-reference.md) — `lns audit`.
