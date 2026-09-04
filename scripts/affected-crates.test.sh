#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$SCRIPT_DIR/affected-crates.sh"

if ! command -v jq >/dev/null 2>&1; then
    echo "skip: affected-crates.sh degrades to __FULL__ without jq, so its crate-list cases cannot run"
    exit 0
fi

# Sourced below the skip: it allocates a scratch home the trap must free.
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

mktmp() {
    mktemp -d "$ROOT/tmp.XXXXXX"
}

init_fixture() {
    dir=$1
    cd "$dir"
    git init -q

    cat > Cargo.toml <<'TOML'
[workspace]
resolver = "2"
members = [
    "crates/lns-cli",
    "crates/lns-ipc",
    "crates/lns-service",
]
TOML

    mkdir -p crates/lns-cli/src
    cat > crates/lns-cli/Cargo.toml <<'TOML'
[package]
name = "lns-cli"
version = "0.1.0"
edition = "2021"

[dependencies]
lns-ipc = { path = "../lns-ipc" }
TOML
    echo "fn main() {}" > crates/lns-cli/src/main.rs

    mkdir -p crates/lns-ipc/src
    cat > crates/lns-ipc/Cargo.toml <<'TOML'
[package]
name = "lns-ipc"
version = "0.1.0"
edition = "2021"
TOML
    echo "" > crates/lns-ipc/src/lib.rs

    mkdir -p crates/lns-service/src
    cat > crates/lns-service/Cargo.toml <<'TOML'
[package]
name = "lns-service"
version = "0.1.0"
edition = "2021"
TOML
    echo "" > crates/lns-service/src/lib.rs

    cat > Cargo.lock <<'TOML'
version = 4

[[package]]
name = "lns-cli"
version = "0.1.0"
dependencies = ["lns-ipc"]

[[package]]
name = "lns-ipc"
version = "0.1.0"

[[package]]
name = "lns-service"
version = "0.1.0"
TOML

    git add -A
    git commit -q -m "base"
}

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

# Test 1: Touch only crates/lns-cli/src/main.rs → lns-cli
test_direct_touch() {
    dir=$(mktmp)
    init_fixture "$dir"
    echo "// changed" >> crates/lns-cli/src/main.rs
    git add -A && git commit -q -m "touch cli"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "direct_touch" "lns-cli" "$actual"
}

# Test 2: Touch only README.md → __NONE__
test_docs_only() {
    dir=$(mktmp)
    init_fixture "$dir"
    echo "# Hello" > README.md
    git add -A && git commit -q -m "add readme"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "docs_only" "__NONE__" "$actual"
}

# Test 3: Touch only .github/workflows/ci.yml → __NONE__
test_github_ci() {
    dir=$(mktmp)
    init_fixture "$dir"
    mkdir -p .github/workflows
    echo "name: CI" > .github/workflows/ci.yml
    git add -A && git commit -q -m "add ci"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "github_ci" "__NONE__" "$actual"
}

# Test 4: Touch Cargo.lock → __FULL__
test_cargo_lock() {
    dir=$(mktmp)
    init_fixture "$dir"
    echo "# modified" >> Cargo.lock
    git add -A && git commit -q -m "touch lock"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "cargo_lock" "__FULL__" "$actual"
}

# Test 5: Touch crates/lns-service/Cargo.toml → list contains lns-service
test_crate_toml() {
    dir=$(mktmp)
    init_fixture "$dir"
    echo "# comment" >> crates/lns-service/Cargo.toml
    git add -A && git commit -q -m "touch service toml"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "crate_toml" "lns-service" "$actual"
}

# Test 6: Touch crates/lns-ipc/src/lib.rs → lns-cli + lns-ipc (rev-dep closure)
test_rev_dep_closure() {
    dir=$(mktmp)
    init_fixture "$dir"
    echo "// changed" >> crates/lns-ipc/src/lib.rs
    git add -A && git commit -q -m "touch ipc"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    expected=$(printf "lns-cli\nlns-ipc")
    assert_eq "rev_dep_closure" "$expected" "$actual"
}

# Test 7: Empty diff (--allow-empty) → __NONE__
test_empty_diff() {
    dir=$(mktmp)
    init_fixture "$dir"
    git commit -q --allow-empty -m "empty"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "empty_diff" "__NONE__" "$actual"
}

# Test 8: Bogus base ref → __FULL__
test_bogus_ref() {
    dir=$(mktmp)
    init_fixture "$dir"
    actual=$("$SCRIPT" "nonexistent-ref-abc123" 2>/dev/null)
    assert_eq "bogus_ref" "__FULL__" "$actual"
}

# Test 9: Mixed — README.md + crates/lns-cli/src/main.rs → lns-cli
test_mixed_docs_and_code() {
    dir=$(mktmp)
    init_fixture "$dir"
    echo "# docs" > README.md
    echo "// changed" >> crates/lns-cli/src/main.rs
    git add -A && git commit -q -m "mixed"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "mixed_docs_and_code" "lns-cli" "$actual"
}

# Test 10: Touch only Makefile → __FULL__
test_root_makefile() {
    dir=$(mktmp)
    init_fixture "$dir"
    echo "# root" > Makefile
    git add -A && git commit -q -m "add makefile"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "root_makefile" "__FULL__" "$actual"
}

# Test 11: Touch only crates/lns-service/Makefile → __FULL__
test_crate_makefile() {
    dir=$(mktmp)
    init_fixture "$dir"
    echo "# crate" > crates/lns-service/Makefile
    git add -A && git commit -q -m "add crate makefile"
    actual=$("$SCRIPT" HEAD~1 2>/dev/null)
    assert_eq "crate_makefile" "__FULL__" "$actual"
}

test_direct_touch
test_docs_only
test_github_ci
test_cargo_lock
test_crate_toml
test_rev_dep_closure
test_empty_diff
test_bogus_ref
test_mixed_docs_and_code
test_root_makefile
test_crate_makefile

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "$FAILURES"
    exit 1
fi
