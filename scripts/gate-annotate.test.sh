#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$SCRIPT_DIR/gate-annotate.sh"

. "$SCRIPT_DIR/test-lib.sh"

PASS=0
FAIL=0
FAILURES=""
ROOT=$(mktemp -d)

cleanup() {
    rm -rf "$ROOT"
    test_lib_cleanup
}
trap cleanup EXIT

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

annotate() {
    status=0
    out=$("$SCRIPT" "$1" 2>&1) || status=$?
    printf '%s\n' "$out"
    return "$status"
}

test_a_failing_file_carries_its_name_and_its_lines() {
    echo "test_a_failing_file_carries_its_name_and_its_lines"
    log=$ROOT/floor.log
    {
        echo "OK    crates/lns-cli/src/output.rs: 100.00% (10/10)"
        echo "FAIL  crates/lns-cli/src/approval/mod.rs: 90.00% (54/60) — must be 100%"
        echo "      uncovered lines: 60,103,122"
    } >"$log"

    status=0
    out=$(annotate "$log") || status=$?
    check "exits 1" "1" "$status"
    check "rows" "2" "$(printf '%s\n' "$out" | grep -c '^::error::')"
    check "names the file" "1" "$(printf '%s\n' "$out" | grep -c 'approval/mod.rs')"
    check "names the lines" "1" "$(printf '%s\n' "$out" | grep -c 'uncovered lines: 60,103,122')"
    check "leaves the passing file out" "0" "$(printf '%s\n' "$out" | grep -c 'output.rs')"
}

test_a_wide_regression_still_says_ten_things() {
    echo "test_a_wide_regression_still_says_ten_things"
    # A `grep | head` pipeline dies of SIGPIPE here and prints nothing at all,
    # which is the failure this script exists to survive. The line count is
    # over the largest stdout buffer seen, because the threshold is the
    # buffer's and not a property of the log.
    log=$ROOT/wide.log
    i=0
    : >"$log"
    while [ "$i" -lt 2000 ]; do
        echo "test tests::case_$i ... FAILED" >>"$log"
        i=$((i + 1))
    done

    status=0
    out=$(annotate "$log") || status=$?
    check "exits 1" "1" "$status"
    check "capped at ten" "10" "$(printf '%s\n' "$out" | grep -c '^::error::')"
    check "names a test" "1" "$(printf '%s\n' "$out" | grep -c 'tests::case_0 ')"
}

test_a_failure_it_cannot_parse_falls_back_to_the_tail() {
    echo "test_a_failure_it_cannot_parse_falls_back_to_the_tail"
    log=$ROOT/compile.log
    {
        echo "   Compiling lns-service v0.24.0"
        echo "error[E0425]: cannot find value \`nope\` in this scope"
        echo "error: could not compile \`lns-service\`"
    } >"$log"

    status=0
    out=$(annotate "$log") || status=$?
    check "exits 1" "1" "$status"
    check "carries the compile error" "1" "$(printf '%s\n' "$out" | grep -c 'E0425')"
}

test_parity_success_chatter_spends_no_slot() {
    echo "test_parity_success_chatter_spends_no_slot"
    log=$ROOT/parity.log
    {
        echo "env-parity: second pass runs as uid 65534"
        echo "env-parity: 26 binaries agree across both environments"
        echo "FAIL  crates/lns-service/src/cache.rs: 99.00% (99/100) — must be 100%"
    } >"$log"

    status=0
    out=$(annotate "$log") || status=$?
    check "exits 1" "1" "$status"
    check "only the failure" "1" "$(printf '%s\n' "$out" | grep -c '^::error::')"
    check "names the file" "1" "$(printf '%s\n' "$out" | grep -c 'cache.rs')"
}

test_a_parity_divergence_is_carried() {
    echo "test_a_parity_divergence_is_carried"
    log=$ROOT/diverged.log
    {
        echo "  FAIL lns_service-98697e8aa9e36732"
        echo "env-parity: these binaries pass in one environment and fail in another: lns_service"
    } >"$log"

    status=0
    out=$(annotate "$log") || status=$?
    check "exits 1" "1" "$status"
    check "rows" "2" "$(printf '%s\n' "$out" | grep -c '^::error::')"
    check "names the binary" "1" "$(printf '%s\n' "$out" | grep -c 'lns_service-98697e8aa9e36732')"
}

test_an_unparseable_failure_keeps_the_line_that_names_it() {
    echo "test_an_unparseable_failure_keeps_the_line_that_names_it"
    # Ten slots is the ceiling GitHub shows, and the deciding line of a compile
    # failure is the last one, so the fallback must keep the tail, not the head.
    log=$ROOT/long-compile.log
    i=0
    : >"$log"
    while [ "$i" -lt 49 ]; do
        echo "   Compiling crate_$i v0.1.0" >>"$log"
        i=$((i + 1))
    done
    echo "error: could not compile \`lns-service\`" >>"$log"

    status=0
    out=$(annotate "$log") || status=$?
    check "exits 1" "1" "$status"
    check "within the ten GitHub shows" "10" "$(printf '%s\n' "$out" | grep -c '^::error::')"
    check "keeps the deciding line" "1" "$(printf '%s\n' "$out" | grep -c 'could not compile')"
}

test_a_gate_that_said_nothing_says_that() {
    echo "test_a_gate_that_said_nothing_says_that"
    log=$ROOT/empty.log
    : >"$log"

    status=0
    out=$(annotate "$log") || status=$?
    check "exits 1" "1" "$status"
    check "one annotation" "1" "$(printf '%s\n' "$out" | grep -c '^::error::')"
    check "says the log is empty" "1" "$(printf '%s\n' "$out" | grep -c 'is empty')"
}

test_help_describes_itself_without_a_log() {
    echo "test_help_describes_itself_without_a_log"
    status=0
    out=$("$SCRIPT" --help 2>&1) || status=$?
    check "exits 0" "0" "$status"
    check "says what it takes" "1" "$(printf '%s\n' "$out" | grep -c 'Usage: scripts/gate-annotate.sh')"
    check "emits no annotation" "0" "$(printf '%s\n' "$out" | grep -c '^::error::')"
}

test_no_argument_names_the_missing_log() {
    echo "test_no_argument_names_the_missing_log"
    status=0
    out=$("$SCRIPT" 2>&1) || status=$?
    check "exits 1" "1" "$status"
    check "says no log" "1" "$(printf '%s\n' "$out" | grep -c 'no gate log at <unset>')"
}

test_a_log_that_is_not_there_says_so() {
    echo "test_a_log_that_is_not_there_says_so"
    status=0
    out=$(annotate "$ROOT/never-written.log") || status=$?
    check "exits 1" "1" "$status"
    check "says which log" "1" "$(printf '%s\n' "$out" | grep -c 'no gate log at')"
}

test_a_failing_file_carries_its_name_and_its_lines
test_a_wide_regression_still_says_ten_things
test_a_failure_it_cannot_parse_falls_back_to_the_tail
test_parity_success_chatter_spends_no_slot
test_a_parity_divergence_is_carried
test_an_unparseable_failure_keeps_the_line_that_names_it
test_a_gate_that_said_nothing_says_that
test_help_describes_itself_without_a_log
test_no_argument_names_the_missing_log
test_a_log_that_is_not_there_says_so

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '%b\n' "$FAILURES"
    exit 1
fi
