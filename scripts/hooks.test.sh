#!/bin/sh
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
HOOKS="$SCRIPT_DIR/hooks"

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

# A repo whose `make` is a stub, so the hook can run without the real gate.
fixture_repo() {
    dir=$(mktmp)
    git -C "$dir" init -q
    git -C "$dir" config user.email t@t.local
    git -C "$dir" config user.name T
    mkdir -p "$dir/scripts/hooks" "$dir/bin"
    cp "$HOOKS/pre-push" "$dir/scripts/hooks/pre-push"
    # The hook must be able to reach the telemetry script, or "no rows" would
    # only prove the script was missing.
    cp "$SCRIPT_DIR/gate-timing.sh" "$dir/scripts/gate-timing.sh"
    cat >"$dir/bin/make" <<'EOF'
#!/bin/sh
exit 0
EOF
    chmod +x "$dir/bin/make"
    git -C "$dir" commit -q --allow-empty -m "seed"
    echo "$dir"
}

# The gate targets time themselves, so a hook that wrapped them too would book
# every step twice and halve the apparent speed-up.
test_the_hook_does_not_double_record() {
    echo "test_the_hook_does_not_double_record"
    dir=$(fixture_repo)
    log=$dir/timings.tsv
    (
        cd "$dir"
        PATH="$dir/bin:$PATH" LNS_GATE_TIMING_LOG="$log" ./scripts/hooks/pre-push </dev/null
    ) >/dev/null 2>&1
    check "the hook records nothing itself" "no" "$([ -f "$log" ] && echo yes || echo no)"
}

test_the_hook_refuses_the_push_when_a_step_fails() {
    echo "test_the_hook_refuses_the_push_when_a_step_fails"
    dir=$(fixture_repo)
    cat >"$dir/bin/make" <<'EOF'
#!/bin/sh
exit 1
EOF
    chmod +x "$dir/bin/make"
    status=0
    (
        cd "$dir"
        PATH="$dir/bin:$PATH" ./scripts/hooks/pre-push </dev/null
    ) >/dev/null 2>&1 || status=$?
    check "exit status" "1" "$status"
}

# The guard exists so a Rust-only checkout can still commit.
test_commit_msg_skips_when_the_tool_is_missing() {
    echo "test_commit_msg_skips_when_the_tool_is_missing"
    dir=$(mktmp)
    git -C "$dir" init -q
    printf 'bogus message\n' >"$dir/msg"
    status=0
    out=$( (cd "$dir" && "$HOOKS/commit-msg" "$dir/msg") 2>&1 ) || status=$?
    check "exit status" "0" "$status"
    check "says why" "1" "$(echo "$out" | grep -c 'npm install to enable')"
}

# node_modules can exist while node itself is unreachable — an nvm shell, a GUI
# git client. That must skip, not refuse the commit.
test_commit_msg_skips_when_the_runtime_is_unreachable() {
    echo "test_commit_msg_skips_when_the_runtime_is_unreachable"
    dir=$(mktmp)
    git -C "$dir" init -q
    mkdir -p "$dir/node_modules/.bin"
    printf '#!/usr/bin/env node\n' >"$dir/node_modules/.bin/commitlint"
    chmod +x "$dir/node_modules/.bin/commitlint"
    printf 'bogus message\n' >"$dir/msg"
    status=0
    out=$( (cd "$dir" && env PATH=/usr/bin:/bin "$HOOKS/commit-msg" "$dir/msg") 2>&1 ) || status=$?
    check "exit status" "0" "$status"
    check "says why" "1" "$(echo "$out" | grep -c 'npm install to enable')"
}

# node_modules is untracked, so a fresh worktree has none. The hook must still
# enforce, or every new worktree would need its own `npm install`.
test_commit_msg_reaches_the_main_worktrees_node_modules() {
    echo "test_commit_msg_reaches_the_main_worktrees_node_modules"
    if ! command -v node >/dev/null 2>&1; then
        echo "  skip (no node on this host)"
        return 0
    fi
    main=$(mktmp)/repo
    mkdir -p "$main/scripts/hooks" "$main/node_modules/.bin"
    git -C "$main" init -q
    git -C "$main" config user.email t@t.local
    git -C "$main" config user.name T
    cp "$HOOKS/commit-msg" "$HOOKS/lib.sh" "$main/scripts/hooks/"
    # Stands in for commitlint: exit 3 is a verdict only this binary can give.
    printf '#!/bin/sh\nexit 3\n' >"$main/node_modules/.bin/commitlint"
    chmod +x "$main/node_modules/.bin/commitlint"
    git -C "$main" -c commit.gpgsign=false commit -q --allow-empty -m seed
    tree=$(mktmp)/wt
    git -C "$main" worktree add -q --detach "$tree" HEAD
    # The seed commit is empty, so the worktree carries no checkout of the hook.
    mkdir -p "$tree/scripts/hooks"
    cp "$HOOKS/commit-msg" "$HOOKS/lib.sh" "$tree/scripts/hooks/"

    printf 'bogus message\n' >"$tree/msg"
    status=0
    (cd "$tree" && ./scripts/hooks/commit-msg msg) >/dev/null 2>&1 || status=$?
    check "the main worktree's tool decided" "3" "$status"
}

# A bare clone driving worktrees puts the git dir beside an unrelated
# directory, whose node_modules must not be mistaken for the repo's.
test_commit_msg_ignores_a_node_modules_outside_any_worktree() {
    echo "test_commit_msg_ignores_a_node_modules_outside_any_worktree"
    if ! command -v node >/dev/null 2>&1; then
        echo "  skip (no node on this host)"
        return 0
    fi
    outside=$(mktmp)
    mkdir -p "$outside/node_modules/.bin"
    # If the hook mistakes this for the repo's, it runs and the test sees 3.
    printf '#!/bin/sh\nexit 3\n' >"$outside/node_modules/.bin/commitlint"
    chmod +x "$outside/node_modules/.bin/commitlint"

    git init -q --bare "$outside/repo.git"
    empty=$(git -C "$outside/repo.git" hash-object -t tree /dev/null)
    seed=$(git -C "$outside/repo.git" commit-tree "$empty" -m seed)
    git -C "$outside/repo.git" update-ref refs/heads/main "$seed"
    tree=$(mktmp)/wt
    git -C "$outside/repo.git" worktree add -q --detach "$tree" main
    mkdir -p "$tree/scripts/hooks"
    cp "$HOOKS/commit-msg" "$HOOKS/lib.sh" "$tree/scripts/hooks/"

    printf 'feat(gate): a perfectly good message\n' >"$tree/msg"
    status=0
    out=$( (cd "$tree" && ./scripts/hooks/commit-msg msg) 2>&1 ) || status=$?
    check "a good message is not refused" "0" "$status"
    check "the outside tool never ran" "1" "$(echo "$out" | grep -c 'npm install to enable')"
}

test_commit_msg_rejects_a_non_conventional_message() {
    echo "test_commit_msg_rejects_a_non_conventional_message"
    if [ ! -x "$SCRIPT_DIR/../node_modules/.bin/commitlint" ] || ! command -v node >/dev/null 2>&1; then
        echo "  skip (commitlint not installed here)"
        return 0
    fi
    dir=$(mktmp)
    printf 'bogus message\n' >"$dir/msg"
    status=0
    "$HOOKS/commit-msg" "$dir/msg" >/dev/null 2>&1 || status=$?
    check "a bad message is refused" "1" "$status"
    printf 'feat(gate): a conventional subject\n' >"$dir/ok"
    status=0
    "$HOOKS/commit-msg" "$dir/ok" >/dev/null 2>&1 || status=$?
    check "a good message passes" "0" "$status"
}

test_the_hook_does_not_double_record
test_the_hook_refuses_the_push_when_a_step_fails
test_commit_msg_skips_when_the_tool_is_missing
test_commit_msg_skips_when_the_runtime_is_unreachable
test_commit_msg_reaches_the_main_worktrees_node_modules
test_commit_msg_ignores_a_node_modules_outside_any_worktree
test_commit_msg_rejects_a_non_conventional_message

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "$FAILURES"
    exit 1
fi
