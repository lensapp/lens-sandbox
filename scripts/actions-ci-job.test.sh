#!/bin/sh
# Where the staging credentials live in ci.yml, and in which order the
# composite actions are exercised. Both are properties of the workflow file,
# so this reads the file rather than running it.
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
WORKFLOW="$SCRIPT_DIR/../.github/workflows/ci.yml"

PASS=0
FAIL=0
FAILURES=""

record() {
    if [ "$2" = yes ]; then
        PASS=$((PASS + 1))
        echo "  ok: $1"
    else
        FAIL=$((FAIL + 1))
        FAILURES="$FAILURES\n  FAIL: $1 — $3"
        echo "  FAIL: $1 — $3"
    fi
}

# Every line of the file prefixed with the job that owns it, and a second
# column saying whether that line is still in the job's header (before its
# `steps:`) — the only place a job-wide `if:` can stand.
annotated() {
    awk '
        /^  [a-z0-9-]+:$/ {
            job = $1
            sub(":", "", job)
            in_steps = 0
            next
        }
        /^    steps:/ { in_steps = 1 }
        { printf "%s\t%s\t%s\t%s\n", NR, job, (in_steps ? "steps" : "header"), $0 }
    ' "$WORKFLOW"
}

job_header_has() {
    annotated | awk -F'\t' -v job="$1" -v needle="$2" \
        '$2 == job && $3 == "header" && index($4, needle) { found = 1 } END { exit !found }'
}

jobs_naming() {
    annotated | awk -F'\t' -v needle="$1" 'index($4, needle) { print $2 }' | sort -u
}

line_of() {
    annotated | awk -F'\t' -v job="$1" -v needle="$2" \
        '$2 == job && index($4, needle) { print $1; exit }'
}

# A pull request can change the composite action code this job loads, so the
# job that carries the staging credentials must be one a pull request cannot
# run at all.
test_staging_credentials_live_only_in_a_main_only_job() {
    holders=$(jobs_naming LNS_HUB_STAGING)
    if [ -z "$holders" ]; then
        record "some job carries the staging credentials" no "no job names LNS_HUB_STAGING"
        return
    fi
    for job in $holders; do
        if job_header_has "$job" "refs/heads/main" &&
            job_header_has "$job" "github.event_name == 'push'"; then
            record "job '$job' carries the staging credentials and only runs on a push to main" yes ""
        else
            record "job '$job' carries the staging credentials and only runs on a push to main" \
                no "its job-level if: does not restrict it to a push to main"
        fi
    done
}

test_the_pull_request_actions_job_names_no_staging_secret() {
    if jobs_naming LNS_HUB_STAGING | grep -qx actions; then
        record "the pull-request actions job names no staging secret" \
            no "the 'actions' job still names LNS_HUB_STAGING"
    else
        record "the pull-request actions job names no staging secret" yes ""
    fi
}

test_the_staging_job_is_gated_on_the_actions_job() {
    if job_header_has actions-staging "needs:" &&
        annotated | awk -F'\t' '$2 == "actions-staging" && $3 == "header" && index($4, "actions")' |
        grep -q .; then
        record "the staging job runs after the actions job" yes ""
    else
        record "the staging job runs after the actions job" no "actions-staging does not need actions"
    fi
}

test_the_aggregate_gate_knows_the_staging_job() {
    if annotated | awk -F'\t' '$2 == "check" && index($4, "actions-staging")' | grep -q .; then
        record "the check gate accounts for the staging job" yes ""
    else
        record "the check gate accounts for the staging job" no "check never mentions actions-staging"
    fi
}

# The suite claims to exercise the actions against the newest release; that is
# only true while the newest release is the lns on PATH when they run.
test_the_actions_run_against_the_newest_release_first() {
    pinned=$(line_of actions "version: 0.25.0")
    login=$(line_of actions "uses: ./actions/lns-login")
    push=$(line_of actions "uses: ./actions/lns-push")
    if [ -z "$pinned" ]; then
        record "the suite still pins a release" no "no pinned setup-lns step"
        return
    fi
    record "the suite still pins a release" yes ""
    for pair in "lns-login:$login" "lns-push:$push"; do
        name=${pair%%:*}
        at=${pair#*:}
        if [ -n "$at" ] && [ "$at" -lt "$pinned" ]; then
            record "$name runs against the newest release" yes ""
        else
            record "$name runs against the newest release" \
                no "its first use is at line ${at:-none}, not before the pinned install at line $pinned"
        fi
    done
}

# …and a pinned CLI is still exercised by the actions, not just by --version.
test_an_action_runs_against_the_pinned_release_too() {
    pinned=$(line_of actions "version: 0.25.0")
    after=$(annotated | awk -F'\t' -v from="$pinned" \
        '$2 == "actions" && $1 > from && index($4, "uses: ./actions/lns-") { print $1; exit }')
    if [ -n "$after" ]; then
        record "an action also runs against the pinned release" yes ""
    else
        record "an action also runs against the pinned release" \
            no "no ./actions/* step follows the pinned install"
    fi
}

echo "── .github/workflows/ci.yml (actions job) ──"
test_staging_credentials_live_only_in_a_main_only_job
test_the_pull_request_actions_job_names_no_staging_secret
test_the_staging_job_is_gated_on_the_actions_job
test_the_aggregate_gate_knows_the_staging_job
test_the_actions_run_against_the_newest_release_first
test_an_action_runs_against_the_pinned_release_too

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '%b\n' "$FAILURES"
    exit 1
fi
