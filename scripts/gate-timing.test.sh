#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$SCRIPT_DIR/gate-timing.sh"

# Every `run` warns when hooks are not installed. That is not what this suite
# is about, and asserting on stderr without it would make `make lint` fail on a
# checkout that has not run `make install-hooks`.
export LNS_GATE_HOOK_WARNING=0

PASS=0
FAIL=0
FAILURES=""
TMPDIRS=""

cleanup() {
    for d in $TMPDIRS; do
        rm -rf "$d"
    done
}
trap cleanup EXIT

mktmp() {
    d=$(mktemp -d)
    TMPDIRS="$TMPDIRS $d"
    echo "$d"
}

check() {
    name=$1
    expected=$2
    actual=$3
    if [ "$expected" = "$actual" ]; then
        PASS=$((PASS + 1))
        echo "  ok   $name"
    else
        FAIL=$((FAIL + 1))
        FAILURES="$FAILURES\n  FAILED $name: expected '$expected', got '$actual'"
        echo "  FAIL $name: expected '$expected', got '$actual'"
    fi
}

test_run_records_a_row() {
    echo "test_run_records_a_row"
    log=$(mktmp)/timings.tsv
    LNS_GATE_TIMING_LOG=$log "$SCRIPT" run lint -- true
    check "one row" "1" "$(grep -c '' "$log")"
    check "step name" "lint" "$(cut -f2 "$log")"
    check "exit code" "0" "$(cut -f4 "$log")"
}

test_run_propagates_failure() {
    echo "test_run_propagates_failure"
    log=$(mktmp)/timings.tsv
    status=0
    LNS_GATE_TIMING_LOG=$log "$SCRIPT" run coverage -- false || status=$?
    check "exit status" "1" "$status"
    check "recorded exit code" "1" "$(cut -f4 "$log")"
}

test_an_unwritable_log_never_fails_the_step() {
    echo "test_an_unwritable_log_never_fails_the_step"
    log=$(mktmp)/is-a-directory
    mkdir -p "$log"

    status=0
    err=$(LNS_GATE_TIMING_LOG=$log "$SCRIPT" run lint -- true 2>&1 >/dev/null) || status=$?
    check "green step stays green" "0" "$status"
    check "no noise on stderr" "" "$err"

    status=0
    LNS_GATE_TIMING_LOG=$log "$SCRIPT" run lint -- false >/dev/null 2>&1 || status=$?
    check "red step stays red" "1" "$status"
}

test_disabled_records_nothing_but_still_runs() {
    echo "test_disabled_records_nothing_but_still_runs"
    dir=$(mktmp)
    log=$dir/timings.tsv
    LNS_GATE_TIMING=0 LNS_GATE_TIMING_LOG=$log "$SCRIPT" run lint -- touch "$dir/ran"
    check "command ran" "yes" "$([ -f "$dir/ran" ] && echo yes || echo no)"
    check "no log" "no" "$([ -f "$log" ] && echo yes || echo no)"
}

test_note_stays_out_of_the_timing_table() {
    echo "test_note_stays_out_of_the_timing_table"
    log=$(mktmp)/timings.tsv
    LNS_GATE_TIMING_LOG=$log "$SCRIPT" note coverage-scope __NONE__
    LNS_GATE_TIMING_LOG=$log "$SCRIPT" run lint -- true
    report=$("$SCRIPT" report "$log")
    check "timing table omits the note" "0" \
        "$(echo "$report" | awk '/^coverage-scope /' | grep -c '')"
    check "scope section keeps it" "1" \
        "$(echo "$report" | grep -c '__NONE__')"
}

test_report_summarises_per_step() {
    echo "test_report_summarises_per_step"
    log=$(mktmp)/timings.tsv
    printf '2026-01-01T00:00:00Z\tlint\t10\t0\tmain\tabc\t\n' >>"$log"
    printf '2026-01-01T00:01:00Z\tlint\t30\t1\tmain\tabc\t\n' >>"$log"
    printf '2026-01-01T00:02:00Z\tlint\t20\t0\tmain\tabc\t\n' >>"$log"
    row=$("$SCRIPT" report --all "$log" | awk '$1 == "lint"')
    check "runs" "3" "$(echo "$row" | awk '{print $2}')"
    check "fails" "1" "$(echo "$row" | awk '{print $3}')"
    check "min" "10" "$(echo "$row" | awk '{print $5}')"
    check "median" "20" "$(echo "$row" | awk '{print $6}')"
    check "max" "30" "$(echo "$row" | awk '{print $7}')"
}

test_report_tolerates_a_missing_log() {
    echo "test_report_tolerates_a_missing_log"
    status=0
    out=$("$SCRIPT" report "$(mktmp)/absent.tsv") || status=$?
    check "exit status" "0" "$status"
    check "explains itself" "1" "$(echo "$out" | grep -c 'run the gate first')"
}

test_a_detail_left_by_the_step_labels_its_row() {
    echo "test_a_detail_left_by_the_step_labels_its_row"
    dir=$(mktmp)
    log=$dir/timings.tsv
    detail=$dir/detail
    LNS_GATE_TIMING_LOG=$log LNS_GATE_DETAIL_FILE=$detail "$SCRIPT" run coverage-data -- \
        env LNS_GATE_TIMING_LOG="$log" LNS_GATE_DETAIL_FILE="$detail" "$SCRIPT" detail coverage-data cold
    check "the row carries the label" "cold" "$(cut -f7 "$log")"
    check "the label is consumed once" "no" "$([ -f "$detail.coverage-data" ] && echo yes || echo no)"

    # A label left for one step must not colour another step's row.
    other=$dir/other.tsv
    LNS_GATE_TIMING_LOG=$other LNS_GATE_DETAIL_FILE=$detail "$SCRIPT" run coverage-data -- \
        env LNS_GATE_TIMING_LOG="$other" LNS_GATE_DETAIL_FILE="$detail" "$SCRIPT" detail lint warm
    check "another step's label is left alone" "" "$(cut -f7 "$other")"
}

test_the_report_window_excludes_older_rows() {
    echo "test_the_report_window_excludes_older_rows"
    log=$(mktmp)/timings.tsv
    printf '2020-01-01T00:00:00Z\tlint\t900\t0\tmain\tabc\t\n' >>"$log"
    printf '%s\tlint\t10\t0\tmain\tabc\t\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" >>"$log"
    windowed=$("$SCRIPT" report "$log" | awk '$1 == "lint" {print $2}')
    check "the default window drops the old row" "1" "$windowed"
    all=$("$SCRIPT" report --all "$log" | awk '$1 == "lint" {print $2}')
    check "--all keeps it" "2" "$all"
}

test_a_dead_hook_warns_without_failing() {
    echo "test_a_dead_hook_warns_without_failing"
    dir=$(mktmp)
    git -C "$dir" init -q
    # The warning reads core.hooksPath, which falls back to the host's global
    # config; pin it so the fixture, not the host, decides.
    git -C "$dir" config core.hooksPath .git/hooks
    log=$dir/timings.tsv
    status=0
    # CI is exempt from the warning, so this case states its own environment
    # rather than inheriting the runner's.
    err=$(cd "$dir" && CI= LNS_GATE_HOOK_WARNING=1 LNS_GATE_TIMING_LOG="$log" \
        "$SCRIPT" run lint -- true 2>&1 >/dev/null) || status=$?
    check "the step still passes" "0" "$status"
    check "it says what to do" "1" "$(echo "$err" | grep -c 'make install-hooks')"
}

# Worktrees are the normal way to work here, and a per-worktree log would
# restart the history on every branch.
test_every_worktree_appends_to_one_log() {
    echo "test_every_worktree_appends_to_one_log"
    main=$(mktmp)/repo
    mkdir -p "$main"
    git -C "$main" init -q
    git -C "$main" config user.email t@t.local
    git -C "$main" config user.name T
    git -C "$main" -c commit.gpgsign=false commit -q --allow-empty -m seed
    tree=$(mktmp)/wt
    git -C "$main" worktree add -q --detach "$tree" HEAD

    # The default path is the subject here, so the fixture must not inherit an
    # override from whoever runs the suite.
    (cd "$main" && env -u LNS_GATE_TIMING_LOG -u LNS_GATE_TIMING "$SCRIPT" run lint -- true)
    (cd "$tree" && env -u LNS_GATE_TIMING_LOG -u LNS_GATE_TIMING "$SCRIPT" run test -- true)

    shared="$main/.git/lns-gate/timings.tsv"
    check "both rows land in the shared log" "2" "$(grep -c '' "$shared" 2>/dev/null || echo 0)"
    check "the worktree keeps no log of its own" "no" \
        "$([ -f "$tree/.gate/timings.tsv" ] && echo yes || echo no)"
}

# git echoes an unknown flag and exits 0, so a naive fallback chain never fires
# and the log lands nowhere at all.
test_an_old_git_still_records_somewhere() {
    echo "test_an_old_git_still_records_somewhere"
    dir=$(mktmp)
    repo=$dir/repo
    mkdir -p "$repo" "$dir/bin"
    git -C "$repo" init -q
    real_git=$(command -v git)
    cat >"$dir/bin/git" <<EOF
#!/bin/sh
# Stands in for git < 2.31, which does not know --path-format: it echoes the
# unrecognised flag back on stdout and still exits 0.
kept=""
seen=""
for a in "\$@"; do
    if [ "\$a" = "--path-format=absolute" ]; then
        seen=yes
    else
        kept="\$kept \$a"
    fi
done
[ -z "\$seen" ] || echo "--path-format=absolute"
exec "$real_git" \$kept
EOF
    chmod +x "$dir/bin/git"

    (
        cd "$repo"
        PATH="$dir/bin:$PATH" env -u LNS_GATE_TIMING_LOG -u LNS_GATE_TIMING \
            "$SCRIPT" run lint -- true
    )
    check "it falls back to the worktree log" "1" \
        "$(grep -c '' "$repo/.gate/timings.tsv" 2>/dev/null || echo 0)"
    check "the row is one line of seven fields" "7" \
        "$(awk -F'\t' 'NR == 1 { print NF }' "$repo/.gate/timings.tsv" 2>/dev/null)"
}

test_run_records_a_row
test_an_old_git_still_records_somewhere
test_every_worktree_appends_to_one_log
test_a_detail_left_by_the_step_labels_its_row
test_the_report_window_excludes_older_rows
test_a_dead_hook_warns_without_failing
test_run_propagates_failure
test_an_unwritable_log_never_fails_the_step
test_disabled_records_nothing_but_still_runs
test_note_stays_out_of_the_timing_table
test_report_summarises_per_step
test_report_tolerates_a_missing_log

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "$FAILURES"
    exit 1
fi
