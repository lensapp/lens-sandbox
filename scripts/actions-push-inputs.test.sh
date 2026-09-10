#!/bin/sh
# What lns-push makes of its `tags`, `push` and `require-exact-tool-versions`
# inputs before anything is validated, dry-run or published.
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$SCRIPT_DIR/../actions/lib/push-inputs.sh"
ACTION="$SCRIPT_DIR/../actions/lns-push/action.yml"

PASS=0
FAIL=0
FAILURES=""
ROOT=$(mktemp -d)

cleanup() {
    rm -rf "$ROOT"
}
trap cleanup EXIT

# Runs the script in a scratch runner: $1 is the tags input, $2 the
# require-exact-tool-versions input, $3 the push input (defaults to `true`).
# Leaves the outputs in $OUT, the tag list
# in $TAGS_FILE, everything the step printed in $LOG, and the status in $STATUS.
run_inputs() {
    work=$(mktemp -d "$ROOT/run.XXXXXX")
    OUT="$work/github-output"
    LOG="$work/log"
    : >"$OUT"
    set +e
    INPUT_TAGS=$1 \
        INPUT_EXACT_TOOLS=$2 \
        INPUT_PUSH=${3-true} \
        RUNNER_TEMP="$work" \
        GITHUB_OUTPUT="$OUT" \
        bash "$SCRIPT" >"$LOG" 2>&1
    STATUS=$?
    set -e
    TAGS_FILE=$(output file || true)
}

output() {
    sed -n "s/^$1=//p" "$OUT" | tail -1
}

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

# A bare reference is the hub's, so it is written down as the hub's: what the
# action probes and reports must be what lns publishes, whatever a reused
# runner has configured as its default registry.
test_bare_tag_is_fully_qualified() {
    run_inputs "acme/hermes:v1" true
    assert_eq "bare tag: status" 0 "$STATUS"
    assert_eq "bare tag: first is fully qualified" \
        "hub.lns.run/acme/hermes:v1" "$(output first)"
    assert_eq "bare tag: host" "hub.lns.run" "$(output host)"
    assert_eq "bare tag: repository" "acme/hermes" "$(output repository)"
    assert_eq "bare tag: the pushed list is fully qualified" \
        "hub.lns.run/acme/hermes:v1" "$(cat "$TAGS_FILE")"
}

test_qualified_tag_is_left_alone() {
    run_inputs "ghcr.io/acme/hermes:v1" true
    assert_eq "qualified tag: first" "ghcr.io/acme/hermes:v1" "$(output first)"
    assert_eq "qualified tag: host" "ghcr.io" "$(output host)"
    assert_eq "qualified tag: repository" "acme/hermes" "$(output repository)"
    assert_eq "qualified tag: the pushed list" \
        "ghcr.io/acme/hermes:v1" "$(cat "$TAGS_FILE")"
}

test_localhost_and_port_stay_hosts() {
    run_inputs "localhost:5000/acme/hermes:v1" true
    assert_eq "localhost:5000: host" "localhost:5000" "$(output host)"
    assert_eq "localhost:5000: repository" "acme/hermes" "$(output repository)"
}

# Surrounding whitespace is the workflow's line wrapping; whitespace inside a
# reference is a malformed reference, and deleting it would publish to a
# repository the author never wrote.
test_only_surrounding_whitespace_is_trimmed() {
    run_inputs "   acme/hermes:v1   " true
    assert_eq "surrounding whitespace is trimmed" \
        "hub.lns.run/acme/hermes:v1" "$(output first)"

    run_inputs "acme/my hermes:v1" true
    assert_eq "inner whitespace survives, so lns can refuse it" \
        "hub.lns.run/acme/my hermes:v1" "$(output first)"
    assert_eq "inner whitespace survives in the pushed list" \
        "hub.lns.run/acme/my hermes:v1" "$(cat "$TAGS_FILE")"
}

test_every_tag_is_kept_in_order() {
    run_inputs "acme/hermes:v1
acme/hermes:ci, acme/hermes:latest" true
    assert_eq "many tags: first" "hub.lns.run/acme/hermes:v1" "$(output first)"
    assert_eq "many tags: the pushed list" \
        "hub.lns.run/acme/hermes:v1
hub.lns.run/acme/hermes:ci
hub.lns.run/acme/hermes:latest" "$(cat "$TAGS_FILE")"
}

# One push, one digest: the dry run of the first tag stands for all of them and
# `digest` is a single output, so every tag has to name the same repository and
# differ only in the tag. A mix is refused before anything is validated.
test_every_tag_must_name_one_repository() {
    run_inputs "acme/hermes:v1
acme/hermes-two:v1" true
    assert_eq "two names: status" 1 "$STATUS"
    case "$(cat "$LOG")" in
        *"hub.lns.run/acme/hermes:v1"*"hub.lns.run/acme/hermes-two:v1"*)
            record "two names: shows the offending pair" yes "" ;;
        *)
            record "two names: shows the offending pair" no "$(cat "$LOG")" ;;
    esac

    run_inputs "acme/hermes:v1
ghcr.io/acme/hermes:v1" true
    assert_eq "two hosts: status" 1 "$STATUS"
    case "$(cat "$LOG")" in
        *"hub.lns.run/acme/hermes:v1"*"ghcr.io/acme/hermes:v1"*)
            record "two hosts: shows the offending pair" yes "" ;;
        *)
            record "two hosts: shows the offending pair" no "$(cat "$LOG")" ;;
    esac

    run_inputs "acme/hermes:v1
acme/hermes-two:v1
acme/hermes-three:v1" true
    case "$(cat "$LOG")" in
        *hermes-three*)
            record "the pair named is the first mismatch" no "$(cat "$LOG")" ;;
        *)
            record "the pair named is the first mismatch" yes "" ;;
    esac

    # One host, written twice with the hub left implicit once: the same
    # repository, so this is not a mix.
    run_inputs "acme/hermes:v1
hub.lns.run/acme/hermes:latest" true
    assert_eq "the hub written out is the same repository: status" 0 "$STATUS"
    assert_eq "the hub written out is the same repository: the pushed list" \
        "hub.lns.run/acme/hermes:v1
hub.lns.run/acme/hermes:latest" "$(cat "$TAGS_FILE")"
}

test_a_reference_without_a_tag_is_refused() {
    run_inputs "acme/hermes" true
    assert_eq "no tag: status" 1 "$STATUS"
    case "$(cat "$LOG")" in
        *"names no tag"*) record "no tag: says so" yes "" ;;
        *) record "no tag: says so" no "$(cat "$LOG")" ;;
    esac
}

test_a_reference_without_a_namespace_is_refused() {
    run_inputs "hermes:v1" true
    assert_eq "no namespace: status" 1 "$STATUS"
    case "$(cat "$LOG")" in
        *"no namespace segment"*) record "no namespace: says so" yes "" ;;
        *) record "no namespace: says so" no "$(cat "$LOG")" ;;
    esac
}

test_no_tags_at_all_is_refused() {
    run_inputs "   " true
    assert_eq "empty tags: status" 1 "$STATUS"
    case "$(cat "$LOG")" in
        *"tags is empty"*) record "empty tags: says so" yes "" ;;
        *) record "empty tags: says so" no "$(cat "$LOG")" ;;
    esac
}

# The safeguard is a switch with two positions. A typo must not read as "off".
test_exact_tool_versions_is_a_boolean() {
    run_inputs "acme/hermes:v1" true
    assert_eq "exact-tools true: status" 0 "$STATUS"
    assert_eq "exact-tools true" true "$(output exact-tools)"

    run_inputs "acme/hermes:v1" false
    assert_eq "exact-tools false: status" 0 "$STATUS"
    assert_eq "exact-tools false" false "$(output exact-tools)"

    run_inputs "acme/hermes:v1" treu
    assert_eq "exact-tools typo: status" 1 "$STATUS"
    case "$(cat "$LOG")" in
        *"require-exact-tool-versions"*"treu"*)
            record "exact-tools typo: names the input and the value" yes "" ;;
        *)
            record "exact-tools typo: names the input and the value" no "$(cat "$LOG")" ;;
    esac

    run_inputs "acme/hermes:v1" ""
    assert_eq "exact-tools empty: status" 1 "$STATUS"
}

# `push` is the same switch: a typo used to read as "off", so the job went
# green having published nothing.
test_push_is_a_boolean() {
    run_inputs "acme/hermes:v1" true true
    assert_eq "push true: status" 0 "$STATUS"
    assert_eq "push true" true "$(output push)"

    run_inputs "acme/hermes:v1" true false
    assert_eq "push false: status" 0 "$STATUS"
    assert_eq "push false" false "$(output push)"

    run_inputs "acme/hermes:v1" true ture
    assert_eq "push typo: status" 1 "$STATUS"
    case "$(cat "$LOG")" in
        *"push"*"ture"*)
            record "push typo: names the input and the value" yes "" ;;
        *)
            record "push typo: names the input and the value" no "$(cat "$LOG")" ;;
    esac

    run_inputs "acme/hermes:v1" true ""
    assert_eq "push empty: status" 1 "$STATUS"
}

# The publish step must read the validated switch, not the raw input.
test_the_publish_step_reads_the_validated_switch() {
    if grep -q "if: steps.tags.outputs.push == 'true'" "$ACTION"; then
        record "the publish step is gated on the validated push output" yes ""
    else
        record "the publish step is gated on the validated push output" no \
            "$(grep -n 'if:' "$ACTION" || echo 'no if: in action.yml')"
    fi
    if grep -q "inputs.push == 'true'" "$ACTION"; then
        record "the publish step no longer reads the raw input" no \
            "action.yml still gates on inputs.push"
    else
        record "the publish step no longer reads the raw input" yes ""
    fi
}

echo "── actions/lib/push-inputs.sh ──"
test_bare_tag_is_fully_qualified
test_qualified_tag_is_left_alone
test_localhost_and_port_stay_hosts
test_only_surrounding_whitespace_is_trimmed
test_every_tag_is_kept_in_order
test_every_tag_must_name_one_repository
test_a_reference_without_a_tag_is_refused
test_a_reference_without_a_namespace_is_refused
test_no_tags_at_all_is_refused
test_exact_tool_versions_is_a_boolean
test_push_is_a_boolean
test_the_publish_step_reads_the_validated_switch

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '%b\n' "$FAILURES"
    exit 1
fi
