#!/bin/sh
# Records how long each verification-gate step takes, and what the
# affected-crates classifier saved.
#
# USAGE:
#   gate-timing.sh run <step> -- <command> [args…]
#   gate-timing.sh note <step> <detail>
#   gate-timing.sh report [<log>]
#
# `run` executes the command, appends one record, and exits with the
# command's status. `note` appends a zero-duration record (used for the
# affected-crates verdict). `report` summarises the log.
#
# The log is TSV at $LNS_GATE_TIMING_LOG (default .gate/timings.tsv, which is
# gitignored and outlives `cargo clean`). Columns:
#
#   started_at  step  duration_s  exit_code  branch  commit  detail
#
# Set LNS_GATE_TIMING=0 to disable recording; `run` still executes the
# command.

set -eu

log_path() {
    if [ -n "${LNS_GATE_TIMING_LOG:-}" ]; then
        echo "$LNS_GATE_TIMING_LOG"
        return
    fi
    root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
    echo "$root/.gate/timings.tsv"
}

# Telemetry never speaks for the step it measures, so it swallows its own I/O errors.
append() {
    [ "${LNS_GATE_TIMING:-1}" = "0" ] && return 0
    log=$(log_path)
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)
    commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
    {
        mkdir -p "$(dirname "$log")" &&
            printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                "$1" "$2" "$3" "$4" "$branch" "$commit" "$5" >>"$log"
    } 2>/dev/null || true
}

cmd_run() {
    step=$1
    shift
    [ "${1:-}" = "--" ] || { echo "gate-timing: expected -- before the command" >&2; exit 2; }
    shift

    started_at=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
    start=$(date '+%s')
    status=0
    "$@" || status=$?
    duration=$(( $(date '+%s') - start ))

    append "$started_at" "$step" "$duration" "$status" "${LNS_GATE_TIMING_DETAIL:-}"
    return $status
}

# A note carries a verdict, not a duration — `-` keeps it out of the timing table.
cmd_note() {
    append "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$1" - 0 "$2"
}

# Per step: runs, failures, total minutes, and the min/median/max seconds.
cmd_report() {
    log=${1:-$(log_path)}
    [ -f "$log" ] || { echo "no timing log at $log — run the gate first"; return 0; }

    echo "gate timings from $log"
    echo ""
    awk -F'\t' '
        $3 ~ /^[0-9]+$/ {
            runs[$2]++
            total[$2] += $3
            if ($4 != 0) failed[$2]++
            times[$2, runs[$2]] = $3
        }
        END {
            printf "%-22s %6s %8s %9s %8s %8s %8s\n", \
                "step", "runs", "fails", "total_min", "min_s", "med_s", "max_s"
            for (step in runs) {
                n = runs[step]
                for (i = 1; i <= n; i++) sorted[i] = times[step, i]
                for (i = 2; i <= n; i++) {
                    v = sorted[i]
                    for (j = i - 1; j >= 1 && sorted[j] > v; j--) sorted[j + 1] = sorted[j]
                    sorted[j + 1] = v
                }
                printf "%-22s %6d %8d %9.1f %8d %8d %8d\n", \
                    step, n, failed[step] + 0, total[step] / 60, \
                    sorted[1], sorted[int((n + 1) / 2)], sorted[n]
            }
        }
    ' "$log"

    echo ""
    echo "coverage scope decisions"
    awk -F'\t' '$2 == "coverage-scope" { scope[$7]++ }
        END { for (s in scope) printf "  %-28s %d\n", s, scope[s] }' "$log"
}

case "${1:-}" in
    run)    shift; cmd_run "$@" ;;
    note)   shift; cmd_note "$@" ;;
    report) shift; cmd_report "$@" ;;
    *)      echo "usage: $0 run <step> -- <cmd…> | note <step> <detail> | report [<log>]" >&2; exit 2 ;;
esac
