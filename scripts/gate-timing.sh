#!/bin/sh
# Records how long each verification-gate step takes, and what the
# affected-crates classifier saved.
#
# USAGE:
#   gate-timing.sh run <step> -- <command> [args…]
#   gate-timing.sh note <step> <detail>
#   gate-timing.sh detail <step> <text>
#   gate-timing.sh report [--since <days> | --all] [<log>]
#
# `run` executes the command, appends one record, and exits with the
# command's status. `note` appends a zero-duration record (used for the
# affected-crates verdict). `detail` labels the row the enclosing `run` will
# write, for something a step only learns about itself mid-run. `report`
# summarises the log, over the last 30 days unless told otherwise.
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

detail_path() {
    if [ -n "${LNS_GATE_DETAIL_FILE:-}" ]; then
        echo "$LNS_GATE_DETAIL_FILE.$1"
    else
        echo "$(dirname "$(log_path)")/detail.$1"
    fi
}

# A step that learns something about itself mid-run — `make coverage` finding a
# cold cache — leaves it here for the wrapper to pick up.
take_detail() {
    file=$(detail_path "$1")
    if [ -f "$file" ]; then
        cat "$file" 2>/dev/null || true
        rm -f "$file" 2>/dev/null || true
    else
        echo "${LNS_GATE_TIMING_DETAIL:-}"
    fi
}

# A dead hook is silent by construction: when hooksPath is unset, no push-time
# code runs to complain. So the gate's own steps carry the warning.
warn_if_hooks_are_dead() {
    [ "${LNS_GATE_HOOK_WARNING:-1}" = "0" ] && return 0
    [ -n "${CI:-}" ] && return 0
    configured=$(git config core.hooksPath 2>/dev/null || echo "")
    [ "$configured" = "scripts/hooks" ] && return 0
    printf '\033[1;33m!\033[0m  git hooks are not installed — run `make install-hooks` (pre-push gate is not running)\n' >&2
}

cmd_run() {
    step=$1
    shift
    [ "${1:-}" = "--" ] || { echo "gate-timing: expected -- before the command" >&2; exit 2; }
    shift

    warn_if_hooks_are_dead
    started_at=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
    start=$(date '+%s')
    status=0
    "$@" || status=$?
    duration=$(( $(date '+%s') - start ))

    append "$started_at" "$step" "$duration" "$status" "$(take_detail "$step")"
    return $status
}

# Called from inside a timed step to label the row the wrapper will write.
cmd_detail() {
    file=$(detail_path "$1")
    { mkdir -p "$(dirname "$file")" && printf '%s' "$2" >"$file"; } 2>/dev/null || true
}

# A note carries a verdict, not a duration — `-` keeps it out of the timing table.
cmd_note() {
    append "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$1" - 0 "$2"
}

# The window's first instant, as an ISO timestamp the rows sort against.
cutoff_for() {
    days=$1
    [ "$days" = "all" ] && { echo ""; return; }
    date -u -d "$days days ago" '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null && return
    date -u -v-"$days"d '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null && return
    echo ""
}

# Per step: runs, failures, total minutes, and the min/median/max seconds.
cmd_report() {
    days=30
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --since)
                [ "$#" -ge 2 ] || { echo "gate-timing: --since needs a number of days" >&2; return 2; }
                days=$2
                shift 2
                ;;
            --all)   days=all; shift ;;
            *)       break ;;
        esac
    done
    log=${1:-$(log_path)}
    [ -f "$log" ] || { echo "no timing log at $log — run the gate first"; return 0; }

    cutoff=$(cutoff_for "$days")
    if [ -n "$cutoff" ]; then
        echo "gate timings from $log (since $cutoff)"
    else
        echo "gate timings from $log (all history)"
    fi
    echo ""
    awk -F'\t' -v cutoff="$cutoff" '
        $1 < cutoff { next }
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
    echo "(coverage-affected covers coverage, which covers coverage-data and parity —"
    echo " the rows nest, so their durations do not sum to wall clock)"

    echo ""
    echo "coverage scope decisions"
    awk -F'\t' -v cutoff="$cutoff" '
        $1 < cutoff { next }
        $2 == "coverage-scope" { scope[$7]++ }
        END { for (s in scope) printf "  %-28s %d\n", s, scope[s] }' "$log"

    # A cold run rebuilt the instrumented tree; averaging the two hides both.
    echo ""
    echo "coverage cache"
    awk -F'\t' -v cutoff="$cutoff" '
        $1 < cutoff { next }
        $2 == "coverage-data" && $3 ~ /^[0-9]+$/ && $7 != "" {
            runs[$7]++
            total[$7] += $3
        }
        END {
            for (kind in runs) {
                printf "  %-10s %3d runs, %6.1f min average\n", \
                    kind, runs[kind], total[kind] / runs[kind] / 60
            }
        }' "$log"
}

case "${1:-}" in
    run)    shift; cmd_run "$@" ;;
    note)   shift; cmd_note "$@" ;;
    report) shift; cmd_report "$@" ;;
    detail) shift; cmd_detail "$@" ;;
    *)      echo "usage: $0 run <step> -- <cmd…> | note <step> <detail> | detail <step> <text> | report [--since <days>|--all] [<log>]" >&2; exit 2 ;;
esac
