#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$SCRIPT_DIR/env-parity.sh"

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

# A stand-in test binary: exits 0, or 1 when the environment says so.
fake_binary() {
    path=$1
    verdict=$2
    cat >"$path" <<EOF
#!/bin/sh
$verdict
EOF
    chmod 0755 "$path"
}

test_a_passing_binary_agrees() {
    echo "test_a_passing_binary_agrees"
    dir=$(mktmp)
    fake_binary "$dir/green" "exit 0"
    status=0
    out=$("$SCRIPT" "$dir/green") || status=$?
    check "exit status" "0" "$status"
    check "counts what ran" "1" "$(echo "$out" | grep -c '1 binaries agree')"
}

test_a_failing_binary_is_named() {
    echo "test_a_failing_binary_is_named"
    dir=$(mktmp)
    fake_binary "$dir/red" "exit 1"
    status=0
    out=$("$SCRIPT" "$dir/red" 2>&1) || status=$?
    check "exit status" "1" "$status"
    check "names it in the summary" "1" "$(echo "$out" | grep -c 'in another:.*red')"
}

test_an_env_dependent_binary_is_caught() {
    echo "test_an_env_dependent_binary_is_caught"
    dir=$(mktmp)
    fake_binary "$dir/tz" '[ "$TZ" = "LNS-14" ] && exit 1; exit 0'
    status=0
    "$SCRIPT" "$dir/tz" >/dev/null 2>&1 || status=$?
    check "the perturbed environment reaches the binary" "1" "$status"
}

test_a_pass_that_runs_nothing_is_not_agreement() {
    echo "test_a_pass_that_runs_nothing_is_not_agreement"
    dir=$(mktmp)
    status=0
    out=$("$SCRIPT" "$dir/never-built" 2>&1) || status=$?
    check "exit status" "1" "$status"
    check "says why" "1" "$(echo "$out" | grep -c 'not executable')"
    check "claims no agreement" "0" "$(echo "$out" | grep -c 'agree across')"
}

test_no_arguments_is_an_error() {
    echo "test_no_arguments_is_an_error"
    status=0
    "$SCRIPT" >/dev/null 2>&1 || status=$?
    check "exit status" "2" "$status"
}

test_the_skip_switch_skips() {
    echo "test_the_skip_switch_skips"
    dir=$(mktmp)
    fake_binary "$dir/red" "exit 1"
    status=0
    LNS_ENV_PARITY=0 "$SCRIPT" "$dir/red" >/dev/null 2>&1 || status=$?
    check "exit status" "0" "$status"
}

# The harness needs traverse permission on the workspace's ancestors; it must
# hand back every bit it takes.
test_widened_directories_are_restored() {
    echo "test_widened_directories_are_restored"
    if [ "$(id -u)" != "0" ]; then
        echo "  skip (needs root to widen anything)"
        return 0
    fi
    # mktemp gives 0700, which the harness must widen to reach the binary and
    # then hand back — assert on that directory, not on one it never touches.
    dir=$(mktmp)
    fake_binary "$dir/green" "exit 0"
    before=$(ls -ld "$dir" | cut -c1-10)
    check "the fixture starts unreachable" "drwx------" "$before"
    "$SCRIPT" "$dir/green" >/dev/null 2>&1 || true
    check "mode handed back" "$before" "$(ls -ld "$dir" | cut -c1-10)"
}

# The Makefile enumerates binaries without jq when jq is absent.
test_the_jqless_enumeration_finds_the_executable() {
    echo "test_the_jqless_enumeration_finds_the_executable"
    line='{"reason":"compiler-artifact","profile":{"opt_level":"0","test":true},"executable":"/t/deps/lns_spec-abc","fresh":true}'
    check "extracts the path" "/t/deps/lns_spec-abc" \
        "$(echo "$line" | sed -n 's/.*"test":true[^{]*"executable":"\([^"]*\)".*/\1/p')"
    nontest='{"reason":"compiler-artifact","profile":{"test":false},"executable":null}'
    check "ignores a non-test artifact" "" \
        "$(echo "$nontest" | sed -n 's/.*"test":true[^{]*"executable":"\([^"]*\)".*/\1/p')"
}

# test-lib.sh pins the toolchain homes from $HOME, which a container uid with
# no passwd entry does not have. It must not become a requirement.
test_the_shared_lib_needs_no_home() {
    echo "test_the_shared_lib_needs_no_home"
    status=0
    out=$(env -u HOME -u CARGO_HOME -u RUSTUP_HOME \
        sh -c "set -u; . \"$SCRIPT_DIR/test-lib.sh\"" 2>&1) || status=$?
    check "sourcing it survives an unset HOME" "0" "$status"
    check "it does not abort on an unbound variable" "0" \
        "$(echo "$out" | grep -c 'parameter not set')"

    # The harnesses that need no toolchain must run in that environment too.
    for t in hooks gate-timing; do
        status=0
        env -u HOME -u CARGO_HOME -u RUSTUP_HOME sh "$SCRIPT_DIR/$t.test.sh" >/dev/null 2>&1 ||
            status=$?
        check "$t survives it" "0" "$status"
    done
}

test_a_passing_binary_agrees
test_the_shared_lib_needs_no_home
test_a_failing_binary_is_named
test_an_env_dependent_binary_is_caught
test_a_pass_that_runs_nothing_is_not_agreement
test_no_arguments_is_an_error
test_the_skip_switch_skips
test_widened_directories_are_restored
test_the_jqless_enumeration_finds_the_executable

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "$FAILURES"
    exit 1
fi
