#!/bin/sh
# Re-emit a failed gate's verdict lines as GitHub annotations, so a reader who
# cannot fetch the job log or the lcov artifact still learns which file, which
# lines, and which tests. Reads the gate's captured output; always exits 1,
# because it only runs on a failure.
#
# Usage: scripts/gate-annotate.sh <log>
#
# Carries the floor's `FAIL <file>` lines and the `uncovered lines:` under each,
# the parity harness's divergence reports, and any failing test names. A failure
# that matches none of them — a compile error, a missing tool — falls back to the
# tail, because a silent annotation step is worse than a noisy one.

set -eu

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
fi

log=${1:-}
if [ -z "$log" ] || [ ! -f "$log" ]; then
    echo "::error::gate-annotate: no gate log at ${log:-<unset>}"
    exit 1
fi

# GitHub shows ten error annotations per step, and a failing file spends two of
# them, so the cap is the ceiling rather than a guess. awk caps in the same
# process that matches: a `grep | head` pipeline dies of SIGPIPE under
# `pipefail`, taking the whole step with it before anything is printed.
said=$(
    awk '
        /^FAIL/ ||
        /^      uncovered lines:/ ||
        /^  FAIL / ||
        /^env-parity: these binaries/ ||
        /^env-parity: no binaries ran/ ||
        /^env-parity: not executable:/ ||
        /^test .*FAILED/ ||
        /^test result: FAILED/ { print; if (++n == 10) exit }
    ' "$log"
)

# The fallback keeps the last ten rather than the first, because the line that
# names the failure — `error: could not compile` — is the last one.
[ -n "$said" ] || said=$(tail -10 "$log")

if [ -z "$said" ]; then
    echo "::error::gate-annotate: $log is empty — the gate said nothing before it failed"
    exit 1
fi

printf '%s\n' "$said" | sed 's/^/::error::/'
exit 1
