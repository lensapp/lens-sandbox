#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
LIB="$SCRIPT_DIR/test-lib.sh"

. "$LIB"

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

# Commits in a repository of its own, exactly as every harness's init_fixture
# does; its TMPDIR sits under ROOT, so the trap frees the repository even when a
# case fails before the fixture can.
write_fixture_script() {
    path=$1
    cat > "$path" <<EOF
#!/bin/sh
set -eu
. "$LIB"
trap test_lib_cleanup EXIT
d=\$(mktemp -d "$ROOT/tmp.XXXXXX")
cd "\$d"
git init -q
echo fixture > file.txt
git add -A
git commit -q -m "fixture commit"
EOF
    chmod +x "$path"
}

outer_repo() {
    outer=$(mktmp)
    git init -q "$outer"
    git -C "$outer" commit -q --allow-empty -m "the caller's own commit"
    echo "$outer"
}

# A linked worktree hands its hooks an absolute GIT_DIR, and with no
# GIT_WORK_TREE git calls the current directory the work tree — so a fixture
# commit lands on the branch being pushed.
test_a_fixture_ignores_the_git_dir_of_its_caller() {
    echo "test_a_fixture_ignores_the_git_dir_of_its_caller"
    outer=$(outer_repo)
    fixture=$(mktmp)/fixture.sh
    write_fixture_script "$fixture"

    status=0
    (cd / && GIT_DIR="$outer/.git" "$fixture") || status=$?

    check "the fixture commits" "0" "$status"
    check "the caller keeps its history" "1" "$(git -C "$outer" rev-list --count HEAD)"
    check "the caller keeps a clean index" "" "$(git -C "$outer" status --porcelain)"
}

test_a_fixture_ignores_the_work_tree_of_its_caller() {
    echo "test_a_fixture_ignores_the_work_tree_of_its_caller"
    outer=$(outer_repo)
    echo outer > "$outer/kept.txt"
    fixture=$(mktmp)/fixture.sh
    write_fixture_script "$fixture"

    status=0
    (cd / && GIT_WORK_TREE="$outer" GIT_DIR="$outer/.git" "$fixture") || status=$?

    check "the fixture commits" "0" "$status"
    check "the caller's tree stays unstaged" "" "$(git -C "$outer" diff --cached --name-only)"
    check "the caller keeps its history" "1" "$(git -C "$outer" rev-list --count HEAD)"
}

test_a_fixture_ignores_the_index_file_of_its_caller() {
    echo "test_a_fixture_ignores_the_index_file_of_its_caller"
    outer=$(outer_repo)
    echo outer > "$outer/kept.txt"
    git -C "$outer" add kept.txt
    fixture=$(mktmp)/fixture.sh
    write_fixture_script "$fixture"

    status=0
    (cd "$outer" && GIT_INDEX_FILE="$outer/.git/index" "$fixture") || status=$?

    check "the fixture commits" "0" "$status"
    check "the caller keeps its staged file" "kept.txt" "$(git -C "$outer" diff --cached --name-only)"
}

# GIT_CONFIG_PARAMETERS carries the caller's `-c` overrides, and it outranks
# both GIT_CONFIG_GLOBAL and GIT_CONFIG_NOSYSTEM.
test_a_fixture_ignores_the_config_overrides_of_its_caller() {
    echo "test_a_fixture_ignores_the_config_overrides_of_its_caller"
    hooks=$(mktmp)
    marker="$ROOT/host-hook-ran"
    printf '#!/bin/sh\ntouch %s\nexit 1\n' "$marker" > "$hooks/pre-commit"
    chmod +x "$hooks/pre-commit"
    fixture=$(mktmp)/fixture.sh
    write_fixture_script "$fixture"

    status=0
    (cd / && GIT_CONFIG_PARAMETERS="'core.hooksPath'='$hooks'" "$fixture") || status=$?

    check "the fixture commits" "0" "$status"
    check "the caller's hook stays out" "absent" "$([ -e "$marker" ] && echo present || echo absent)"
}

test_a_fixture_ignores_the_git_dir_of_its_caller
test_a_fixture_ignores_the_work_tree_of_its_caller
test_a_fixture_ignores_the_index_file_of_its_caller
test_a_fixture_ignores_the_config_overrides_of_its_caller

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "$FAILURES"
    exit 1
fi
