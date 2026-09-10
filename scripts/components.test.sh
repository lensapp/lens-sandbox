#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="${LNS_COMPONENTS_SCRIPT:-$SCRIPT_DIR/components.sh}"

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

assert_eq() {
    test_name=$1
    expected=$2
    actual=$3
    if [ "$expected" = "$actual" ]; then
        PASS=$((PASS + 1))
        echo "OK $test_name"
    else
        FAIL=$((FAIL + 1))
        FAILURES="$FAILURES
  FAIL $test_name — expected '$expected', got '$actual'"
        echo "FAIL $test_name — expected '$expected', got '$actual'"
    fi
}

# A stub in place of a real build script, so this suite needs no wasm toolchain.
stub() {
    path="$ROOT/$1"
    printf '#!/bin/sh\necho "%s ran $*"\nexit %s\n' "$1" "$2" > "$path"
    chmod +x "$path"
    echo "$path"
}

# `components.sh` resolves `rustup` by name, so a stub first on PATH decides
# what it believes is installed. Every case then runs on every host, instead of
# each branch running only where the runner happens to agree with it.
BIN="$ROOT/bin"
mkdir -p "$BIN"
printf '#!/bin/sh\necho wasm32-wasip2\n' > "$BIN/rustup"
chmod +x "$BIN/rustup"
export PATH="$BIN:$PATH"

# The stub is a real file every later case reads, so it is restored whatever the
# inner command did: a case that failed here would otherwise leave every case
# after it believing the target is missing.
without_the_target() {
    printf '#!/bin/sh\nexit 0\n' > "$BIN/rustup"
    status=0
    "$@" || status=$?
    printf '#!/bin/sh\necho wasm32-wasip2\n' > "$BIN/rustup"
    return "$status"
}

status_of() {
    env -u CI "$SCRIPT" "$@" >/dev/null 2>&1 && echo 0 || echo 1
}

in_ci() {
    CI=1 "$SCRIPT" "$@" >/dev/null 2>&1 && echo 0 || echo 1
}

# Test 1: every script passing is a pass.
test_all_pass() {
    assert_eq "all_pass" "0" "$(status_of "$(stub first 0)" "$(stub second 0)")"
}

# Test 2: the first script failing must fail the step even though the last one
# passed. A shell that took the last status would discard it, and a stale
# component would reach main behind a green gate.
test_first_failure_is_not_swallowed() {
    assert_eq "first_failure_is_not_swallowed" "1" \
        "$(status_of "$(stub first 1)" "$(stub second 0)")"
}

# Test 3: a later script failing fails the step too.
test_last_failure_fails() {
    assert_eq "last_failure_fails" "1" "$(status_of "$(stub first 0)" "$(stub second 1)")"
}

# Test 4: every script runs, so one failure does not hide the next.
test_every_script_runs() {
    first=$(stub first 1)
    second=$(stub second 1)
    out=$(env -u CI "$SCRIPT" "$first" "$second" 2>&1 || true)
    case "$out" in
        *"first ran --check"*"second ran --check"*) ran="both" ;;
        *) ran="$out" ;;
    esac
    assert_eq "every_script_runs" "both" "$ran"
}

# Test 5: naming no script at all is a usage error, not a silent pass.
test_no_arguments() {
    assert_eq "no_arguments" "1" "$(status_of)"
}

# Test 6: without the target the script skips before it runs anything, so even a
# script that would fail is never reached.
test_a_skip_runs_nothing() {
    assert_eq "a_skip_runs_nothing" "0" "$(without_the_target status_of "$(stub first 1)")"
}

# Test 7: CI installs the target, so a skip there is a dead step and must fail —
# silence must not read as agreement.
test_a_skip_in_ci_is_a_dead_step() {
    assert_eq "a_skip_in_ci_is_a_dead_step" "1" \
        "$(without_the_target in_ci "$(stub first 0)")"
}

test_all_pass
test_first_failure_is_not_swallowed
test_last_failure_fails
test_every_script_runs
test_no_arguments
test_a_skip_runs_nothing
test_a_skip_in_ci_is_a_dead_step

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "$FAILURES"
    exit 1
fi
