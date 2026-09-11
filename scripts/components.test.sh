#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="${LNS_COMPONENTS_SCRIPT:-$SCRIPT_DIR/components.sh}"

. "$SCRIPT_DIR/test-lib.sh"

PASS=0
FAIL=0
FAILURES=""
# One directory down, so a case can put something beside the suite and assert
# that nothing the suite runs reaches it.
OUTSIDE=$(mktemp -d)
ROOT="$OUTSIDE/suite"

cleanup() {
    rm -rf "$OUTSIDE"
    test_lib_cleanup
}
trap cleanup EXIT
mkdir "$ROOT"

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
# It sits one directory below the component it writes, which is the real scripts'
# shape and keeps every scan inside the directory the case owns.
stub() {
    dir="$ROOT/$1"
    mkdir -p "$dir/mechanism"
    path="$dir/mechanism/build.sh"
    printf '#!/bin/sh\necho "%s ran $*"\nexit %s\n' "$1" "$2" > "$path"
    chmod +x "$path"
    printf '%s' "${3:-/rustc/8bab26f4/library/core/src/fmt/mod.rs}" > "$dir/component.wasm"
    echo "$path"
}

# A build script whose components are not where it writes them.
stub_writing_nothing() {
    dir="$ROOT/$1"
    mkdir -p "$dir/mechanism"
    path="$dir/mechanism/build.sh"
    printf '#!/bin/sh\nexit 0\n' > "$path"
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

# Test 8: a committed component that embeds the path this machine keeps its
# registry at is bytes no other machine rebuilds, so `cmp` agrees here and
# nowhere else. The step fails and names the file.
test_a_component_that_embeds_this_machine_fails() {
    script=$(stub leaky 0 'x/root/dev/.cargo/registry/src/serde.rs')
    out=$(env -u CI "$SCRIPT" "$script" 2>&1 || true)
    case "$out" in
        *component.wasm*) named="named" ;;
        *) named="$out" ;;
    esac
    assert_eq "a_component_that_embeds_this_machine_is_named" "named" "$named"
    assert_eq "a_component_that_embeds_this_machine_fails" "1" "$(status_of "$script")"
}

# Test 9: the paths a component legitimately carries are the toolchain's own,
# which every host running the pin writes identically. The guard leaves them be.
test_the_toolchain_s_own_paths_pass() {
    assert_eq "the_toolchain_s_own_paths_pass" "0" \
        "$(status_of "$(stub tidy 0 'x/rustc/8bab26f4/library/core/src/fmt/mod.rs')")"
}

# Test 10: the scan reads the components a script writes and nothing else. A
# scan that climbed to whatever sits beside them would report the host, and
# TMPDIR is not this suite's to answer for.
test_the_scan_stays_beside_the_script() {
    printf '%s' '/home/someone/.cargo/registry/src/serde.rs' > "$OUTSIDE/stray.wasm"
    assert_eq "the_scan_stays_beside_the_script" "0" "$(status_of "$(stub tidied 0)")"
}

# Test 11: a script that wrote its components somewhere the scan does not look
# leaves the scan reading nothing, and silence must not read as agreement.
test_scanning_nothing_is_a_dead_step() {
    assert_eq "scanning_nothing_is_a_dead_step" "1" \
        "$(status_of "$(stub_writing_nothing empty)")"
}

# Test 12: a run that failed only the scan still names the script that wrote the
# components, so the summary line is never bare.
test_a_leak_names_the_script() {
    script=$(stub named 0 'x/root/dev/.cargo/registry/src/serde.rs')
    out=$(env -u CI "$SCRIPT" "$script" 2>&1 || true)
    case "$out" in
        *"components failed: $script"*) named="named" ;;
        *) named="$out" ;;
    esac
    assert_eq "a_leak_names_the_script" "named" "$named"
}

# Test 13: a host that keeps its registry outside a home directory leaks past
# the pattern, so the scan also looks for the path this machine builds under.
test_this_machine_s_registry_is_looked_for() {
    script=$(stub opted 0 'x/opt/cargo/registry/src/index.crates.io/serde.rs')
    out=$(env -u CI CARGO_HOME=/opt/cargo "$SCRIPT" "$script" 2>&1 || true)
    case "$out" in
        *component.wasm*) named="named" ;;
        *) named="$out" ;;
    esac
    assert_eq "this_machine_s_registry_is_named" "named" "$named"
    assert_eq "this_machine_s_registry_is_looked_for" "1" \
        "$(env -u CI CARGO_HOME=/opt/cargo "$SCRIPT" "$script" >/dev/null 2>&1 && echo 0 || echo 1)"
}

# Test 14: a host that keeps its registry at the path the build remaps onto
# rebuilds the very bytes the scan would call a leak, so it would have no way
# back to green. What the remap writes is what every host writes.
test_the_remap_target_is_not_a_leak() {
    script=$(stub remapped 0 'x/cargo/registry/src/index.crates.io/serde.rs')
    assert_eq "the_remap_target_is_not_a_leak" "0" \
        "$(env -u CI CARGO_HOME=/cargo "$SCRIPT" "$script" >/dev/null 2>&1 && echo 0 || echo 1)"
    # rustc strips the trailing slash, so the bytes read the same and a host that
    # spells its registry with one must not be told otherwise.
    assert_eq "the_remap_target_with_a_trailing_slash_is_not_a_leak" "0" \
        "$(env -u CI CARGO_HOME=/cargo/ "$SCRIPT" "$script" >/dev/null 2>&1 && echo 0 || echo 1)"
}

test_all_pass
test_first_failure_is_not_swallowed
test_last_failure_fails
test_every_script_runs
test_no_arguments
test_a_skip_runs_nothing
test_a_skip_in_ci_is_a_dead_step
test_a_component_that_embeds_this_machine_fails
test_the_toolchain_s_own_paths_pass
test_the_scan_stays_beside_the_script
test_scanning_nothing_is_a_dead_step
test_a_leak_names_the_script
test_this_machine_s_registry_is_looked_for
test_the_remap_target_is_not_a_leak

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "$FAILURES"
    exit 1
fi
