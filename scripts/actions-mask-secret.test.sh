#!/bin/sh
# What lns-login emits to hide the password: the runner decodes a workflow
# command's data, so the value has to reach `add-mask` escaped or it is neither
# masked as entered nor kept out of the log.
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$SCRIPT_DIR/../actions/lib/mask-secret.sh"
ACTION="$SCRIPT_DIR/../actions/lns-login/action.yml"

PASS=0
FAIL=0
FAILURES=""

record() {
    name=$1
    ok=$2
    detail=$3
    if [ "$ok" = yes ]; then
        PASS=$((PASS + 1))
        echo "  ok: $name"
    else
        FAIL=$((FAIL + 1))
        FAILURES="$FAILURES\n  FAIL: $name — $detail"
        echo "  FAIL: $name — $detail"
    fi
}

assert_eq() {
    name=$1
    expected=$2
    actual=$3
    if [ "$expected" = "$actual" ]; then
        record "$name" yes ""
    else
        record "$name" no "expected '$expected', got '$actual'"
    fi
}

# Runs the script with $1 as the password, leaving everything it printed in
# $LOG and the status in $STATUS.
run_mask() {
    set +e
    LOG=$(LNS_ACTION_SECRET=$1 bash "$SCRIPT" 2>&1)
    STATUS=$?
    set -e
}

test_a_plain_password_is_masked_as_entered() {
    run_mask "s3cret"
    assert_eq "plain: status" 0 "$STATUS"
    assert_eq "plain: masked as entered" "::add-mask::s3cret" "$LOG"
}

# `%` starts the runner's escape, so an unescaped one makes it read the rest of
# the password as an escape sequence and mask something the user never typed.
test_a_percent_is_escaped() {
    run_mask "100%pass"
    assert_eq "percent: masked with the percent escaped" \
        "::add-mask::100%25pass" "$LOG"
}

# A password that literally contains %0A must be masked as those four
# characters, not as the newline the runner would decode them to.
test_a_literal_escape_sequence_is_not_decoded() {
    run_mask "a%0Ab%0Dc"
    assert_eq "literal %0A/%0D: escaped, not decoded" \
        "::add-mask::a%250Ab%250Dc" "$LOG"
}

# A newline ends the workflow command: unescaped, every later line of the
# password is printed to the log as an ordinary line.
test_a_newline_never_reaches_the_log() {
    run_mask "line-one
line-two"
    assert_eq "newline: status" 0 "$STATUS"
    assert_eq "newline: one command, no second line in the log" \
        "::add-mask::line-one%0Aline-two" "$LOG"
    case "$LOG" in
        *line-two*"%0A"*) record "newline: the later line is not printed bare" no "$LOG" ;;
        *"%0Aline-two") record "newline: the later line is not printed bare" yes "" ;;
        *) record "newline: the later line is not printed bare" no "$LOG" ;;
    esac
}

test_a_carriage_return_is_escaped() {
    run_mask "$(printf 'a\rb')"
    assert_eq "carriage return: escaped" "::add-mask::a%0Db" "$LOG"
}

test_an_empty_password_is_refused() {
    run_mask ""
    assert_eq "empty: status" 1 "$STATUS"
    case "$LOG" in
        *"::error::"*password*) record "empty: names the input" yes "" ;;
        *) record "empty: names the input" no "$LOG" ;;
    esac
}

# The action must go through the script; an inline `echo ::add-mask::` is the
# unescaped emission this harness exists to keep out.
test_the_action_masks_through_the_script() {
    if grep -q 'lib/mask-secret.sh' "$ACTION"; then
        record "the action masks through the script" yes ""
    else
        record "the action masks through the script" no \
            "$(grep -n 'add-mask' "$ACTION" || echo 'no add-mask in action.yml')"
    fi
    if grep -q 'add-mask::\$' "$ACTION"; then
        record "the action no longer emits the raw password" no \
            "action.yml still echoes the password into add-mask"
    else
        record "the action no longer emits the raw password" yes ""
    fi
}

echo "── actions/lib/mask-secret.sh ──"
test_a_plain_password_is_masked_as_entered
test_a_percent_is_escaped
test_a_literal_escape_sequence_is_not_decoded
test_a_newline_never_reaches_the_log
test_a_carriage_return_is_escaped
test_an_empty_password_is_refused
test_the_action_masks_through_the_script

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '%b\n' "$FAILURES"
    exit 1
fi
