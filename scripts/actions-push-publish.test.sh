#!/bin/sh
# What lns-push reports after publishing: one `digest` output describes every
# reference in `refs`, so the loop has to prove every tag received the same
# manifest rather than reporting the last push's digest beside all of them.
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$SCRIPT_DIR/../actions/lib/push-publish.sh"
ACTION="$SCRIPT_DIR/../actions/lns-push/action.yml"

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

assert_contains() {
    name=$1
    haystack=$2
    needle=$3
    case "$haystack" in
        *"$needle"*) record "$name" yes "" ;;
        *) record "$name" no "'$needle' missing from: $haystack" ;;
    esac
}

assert_lacks() {
    name=$1
    haystack=$2
    needle=$3
    case "$haystack" in
        *"$needle"*) record "$name" no "'$needle' present in: $haystack" ;;
        *) record "$name" yes "" ;;
    esac
}

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

# A stand-in `lns` whose push reports one scripted digest per call, and a
# stand-in `curl` whose probe reports one scripted status: what the script has
# to decide from is exactly those two answers.
make_fakes() {
    digests=$1
    http=$2
    rm -rf "${WORK:?}/bin"
    mkdir -p "$WORK/bin"
    printf '%s\n' "$digests" >"$WORK/digests"
    : >"$WORK/calls"
    cat >"$WORK/bin/lns" <<'FAKE'
#!/bin/sh
tag=$3
printf '%s\n' "$tag" >>"$WORK/calls"
count=$(wc -l <"$WORK/calls" | tr -d ' ')
digest=$(sed -n "${count}p" "$WORK/digests")
case "$digest" in
    FAIL) echo "refused $tag"; exit 7 ;;
    NONE) echo "nothing to report for $tag"; exit 0 ;;
esac
echo "built and pushed $tag@$digest"
FAKE
    cat >"$WORK/bin/curl" <<FAKE
#!/bin/sh
echo "$http"
FAKE
    chmod +x "$WORK/bin/lns" "$WORK/bin/curl"
}

# Runs the script over \$1 (newline-separated tags), leaving everything it
# printed in \$LOG, the status in \$STATUS, the step outputs in \$OUTPUT and the
# job summary in \$SUMMARY.
run_publish() {
    tags=$1
    printf '%s\n' "$tags" >"$WORK/tags.txt"
    : >"$WORK/output"
    : >"$WORK/summary"
    set +e
    LOG=$(
        export WORK
        export PATH="$WORK/bin:$PATH"
        export INPUT_FILE=lns.yaml
        export TAGS_FILE="$WORK/tags.txt"
        export REGISTRY_HOST=hub.lns.run
        export REPOSITORY=acme/gh
        export GITHUB_OUTPUT="$WORK/output"
        export GITHUB_STEP_SUMMARY="$WORK/summary"
        bash "$SCRIPT" 2>&1
    )
    STATUS=$?
    set -e
    OUTPUT=$(cat "$WORK/output")
    SUMMARY=$(cat "$WORK/summary")
}

test_one_tag_reports_its_digest() {
    make_fakes "sha256:aaa" 200
    run_publish "hub.lns.run/acme/gh:v1"
    assert_eq "one tag: status" 0 "$STATUS"
    assert_contains "one tag: digest output" "$OUTPUT" "digest=sha256:aaa"
    assert_contains "one tag: refs output" "$OUTPUT" "hub.lns.run/acme/gh:v1"
    assert_contains "one tag: first-push false" "$OUTPUT" "first-push=false"
}

test_tags_sharing_a_manifest_report_one_digest() {
    make_fakes "sha256:aaa
sha256:aaa" 200
    run_publish "hub.lns.run/acme/gh:v1
hub.lns.run/acme/gh:latest"
    assert_eq "same manifest: status" 0 "$STATUS"
    assert_contains "same manifest: digest output" "$OUTPUT" "digest=sha256:aaa"
    assert_contains "same manifest: first ref" "$OUTPUT" "hub.lns.run/acme/gh:v1"
    assert_contains "same manifest: second ref" "$OUTPUT" "hub.lns.run/acme/gh:latest"
}

# With `require-exact-tool-versions: false` each push resolves fuzzy tools on
# its own, so two tags can receive two manifests. One `digest` output cannot
# describe that, and reporting the last one beside both refs is a lie.
test_a_second_manifest_fails_the_step() {
    make_fakes "sha256:aaa
sha256:bbb" 200
    run_publish "hub.lns.run/acme/gh:v1
hub.lns.run/acme/gh:latest"
    assert_eq "two manifests: status" 1 "$STATUS"
    assert_contains "two manifests: names the first ref" "$LOG" "hub.lns.run/acme/gh:v1"
    assert_contains "two manifests: names the first digest" "$LOG" "sha256:aaa"
    assert_contains "two manifests: names the second ref" "$LOG" "hub.lns.run/acme/gh:latest"
    assert_contains "two manifests: names the second digest" "$LOG" "sha256:bbb"
    assert_contains "two manifests: is an error annotation" "$LOG" "::error::"
    assert_lacks "two manifests: no digest is reported" "$OUTPUT" "digest=sha256:"
    assert_contains "two manifests: the summary says what was published" \
        "$SUMMARY" "sha256:bbb"
}

# The digest is read out of the push's own report; without it there is nothing
# truthful to put in the output, and an empty one used to be published as fact.
test_a_push_reporting_no_digest_fails_the_step() {
    make_fakes "NONE" 200
    run_publish "hub.lns.run/acme/gh:v1"
    assert_eq "no digest: status" 1 "$STATUS"
    assert_contains "no digest: names the ref" "$LOG" "hub.lns.run/acme/gh:v1"
    assert_lacks "no digest: nothing is reported" "$OUTPUT" "digest="
}

test_a_failed_push_stops_the_loop() {
    make_fakes "sha256:aaa
FAIL" 200
    run_publish "hub.lns.run/acme/gh:v1
hub.lns.run/acme/gh:latest"
    assert_eq "failed push: status" 7 "$STATUS"
    assert_contains "failed push: names the ref" "$LOG" "pushing hub.lns.run/acme/gh:latest"
}

test_a_missing_repository_is_a_first_push() {
    make_fakes "sha256:aaa" 404
    run_publish "hub.lns.run/acme/gh:v1"
    assert_eq "first push: status" 0 "$STATUS"
    assert_contains "first push: output" "$OUTPUT" "first-push=true"
    assert_contains "first push: the summary points at the settings page" \
        "$SUMMARY" "hub.lns.run/acme/gh/settings"
}

# The action must go through the script; an inline loop is what this harness
# exists to keep out.
test_the_action_publishes_through_the_script() {
    if grep -q 'lib/push-publish.sh' "$ACTION"; then
        record "the action publishes through the script" yes ""
    else
        record "the action publishes through the script" no \
            "$(grep -n 'artifact push' "$ACTION" || echo 'no push in action.yml')"
    fi
    if grep -q 'artifact push "[$]tag"' "$ACTION"; then
        record "the action no longer loops over the tags inline" no \
            "action.yml still pushes each tag inline"
    else
        record "the action no longer loops over the tags inline" yes ""
    fi
    if grep -q "if: steps.tags.outputs.push == 'true'" "$ACTION"; then
        record "the publish step is still gated on the validated push output" yes ""
    else
        record "the publish step is still gated on the validated push output" no \
            "$(grep -n 'if:' "$ACTION" || echo 'no if: in action.yml')"
    fi
}

echo "── actions/lib/push-publish.sh ──"
test_one_tag_reports_its_digest
test_tags_sharing_a_manifest_report_one_digest
test_a_second_manifest_fails_the_step
test_a_push_reporting_no_digest_fails_the_step
test_a_failed_push_stops_the_loop
test_a_missing_repository_is_a_first_push
test_the_action_publishes_through_the_script

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '%b\n' "$FAILURES"
    exit 1
fi
