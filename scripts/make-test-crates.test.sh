#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

# The target under test wraps itself in gate telemetry, and every timed step
# warns when hooks are not installed. That warning is not this suite's subject.
export LNS_GATE_HOOK_WARNING=0

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

# A stand-in for cargo: it answers the `metadata` the Makefile reads at parse
# time and records every other argv instead of building anything.
write_fake_cargo() {
    cat >"$1" <<'EOF'
#!/bin/sh
if [ "$1" = metadata ]; then
    printf '{"workspace_root":"%s","target_directory":"%s/target"}\n' \
        "$FAKE_WORKSPACE" "$FAKE_WORKSPACE"
    exit 0
fi
printf '%s\n' "$*" >>"$FAKE_ARGV"
EOF
    chmod +x "$1"
}

run_target() {
    dir=$(mktemp -d "$ROOT/case.XXXXXX")
    write_fake_cargo "$dir/cargo"
    ARGV="$dir/argv"
    LOG="$dir/timings.tsv"
    STDERR="$dir/stderr"
    STATUS=0
    FAKE_WORKSPACE="$dir" FAKE_ARGV="$ARGV" \
        LNS_GATE_TIMING_LOG="$LOG" LNS_GATE_DETAIL_FILE="$dir/detail" \
        LNS_GATE_TIMING=1 \
        make -C "$REPO_ROOT" --no-print-directory CARGO="$dir/cargo" CARGO_LOCKED= "$@" \
        >/dev/null 2>"$STDERR" || STATUS=$?
}

test_it_maps_crates_to_package_flags() {
    echo "test_it_maps_crates_to_package_flags"
    run_target test-crates CRATES="lns-ipc lns-cli"
    check "exit status" "0" "$STATUS"
    check "cargo argv" "test -p lns-ipc -p lns-cli --all-targets" "$(cat "$ARGV")"
}

test_it_records_one_timed_row_naming_the_crates() {
    echo "test_it_records_one_timed_row_naming_the_crates"
    run_target test-crates CRATES="lns-policy"
    check "one row" "1" "$(grep -c '' "$LOG")"
    check "step name" "test-crates" "$(cut -f2 "$LOG")"
    check "recorded exit code" "0" "$(cut -f4 "$LOG")"
    check "the crates label the row" "lns-policy" "$(cut -f7 "$LOG")"
}

test_it_passes_ci_strictness_through() {
    echo "test_it_passes_ci_strictness_through"
    run_target test-crates CRATES="lns-spec" CARGO_LOCKED="--locked"
    check "cargo sees the strictness flag" \
        "test -p lns-spec --all-targets --locked" "$(cat "$ARGV")"
}

# CI exports CARGO_LOCKED for `make lint`, which runs this harness. The
# fixture states its own scope, or the assertions read the host.
test_it_ignores_an_inherited_strictness_flag() {
    echo "test_it_ignores_an_inherited_strictness_flag"
    CARGO_LOCKED=--locked run_target test-crates CRATES="lns-ipc"
    check "cargo argv" "test -p lns-ipc --all-targets" "$(cat "$ARGV")"
}

test_it_refuses_the_layer_1_crate() {
    echo "test_it_refuses_the_layer_1_crate"
    run_target test-crates CRATES="lns-cli e2e-tests"
    check "exit status" "2" "$STATUS"
    check "cargo test never runs" "0" "$(grep -c '' "$ARGV" 2>/dev/null || echo 0)"
    check "it names the target that owns Layer 1" "1" "$(grep -c 'make e2e' "$STDERR")"
}

test_it_refuses_an_empty_crate_list() {
    echo "test_it_refuses_an_empty_crate_list"
    run_target test-crates
    check "exit status" "2" "$STATUS"
    check "cargo test never runs" "0" "$(grep -c '' "$ARGV" 2>/dev/null || echo 0)"
    check "it names the variable" "1" "$(grep -c 'CRATES=' "$STDERR")"
}

test_it_maps_crates_to_package_flags
test_it_records_one_timed_row_naming_the_crates
test_it_passes_ci_strictness_through
test_it_ignores_an_inherited_strictness_flag
test_it_refuses_the_layer_1_crate
test_it_refuses_an_empty_crate_list

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "$FAILURES"
    exit 1
fi
