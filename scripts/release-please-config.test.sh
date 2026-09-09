#!/bin/sh
# What release-please-config.json promises about which commits release what.
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
CONFIG="$SCRIPT_DIR/../release-please-config.json"

if ! command -v jq >/dev/null 2>&1; then
    echo "skip: release-please-config.test.sh reads the config with jq, which is not on PATH"
    exit 0
fi

PASS=0
FAIL=0
FAILURES=""

assert_ok() {
    name=$1
    program=$2
    if jq -e "$program" "$CONFIG" >/dev/null; then
        PASS=$((PASS + 1))
        echo "  ok: $name"
    else
        FAIL=$((FAIL + 1))
        FAILURES="$FAILURES\n  FAIL: $name"
        echo "  FAIL: $name"
    fi
}

# The root package releases the lns binary. A commit that only touches paths
# another package owns must not open an lns release pull request.
test_root_excludes_actions() {
    assert_ok "root package excludes actions/" \
        '.packages["."]["exclude-paths"] | index("actions")'
}

test_root_excludes_codeowners() {
    assert_ok "root package excludes CODEOWNERS" \
        '.packages["."]["exclude-paths"] | index("CODEOWNERS")'
}

# Every root-level file that governs the repository rather than the binary.
test_root_excludes_every_governance_file() {
    # shellcheck disable=SC2016 # a jq program, not shell expansion
    assert_ok "root package excludes every root-level governance file" \
        '(.packages["."]["exclude-paths"]) as $e
         | ["CODEOWNERS", ".github", ".vscode", "docs"]
         | all(. as $p | $e | index($p) != null)'
}

test_actions_package_is_its_own_component() {
    assert_ok "actions is released as lns-actions" \
        '.packages["actions"].component == "lns-actions"'
}

echo "── release-please-config.json ──"
test_root_excludes_actions
test_root_excludes_codeowners
test_root_excludes_every_governance_file
test_actions_package_is_its_own_component

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '%b\n' "$FAILURES"
    exit 1
fi
