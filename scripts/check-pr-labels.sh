#!/usr/bin/env bash
# Enforce the PR label policy: exactly one type label, and no labels outside the
# sanctioned vocabulary. Reads the PR's label names as a JSON array on stdin.
#
# Usage: echo '["bug","dependencies"]' | scripts/check-pr-labels.sh
#
# Type labels (exactly one required): enhancement bug documentation chore
#   refactor test ci release.
# Auxiliary labels (tolerated as extras): dependencies, autorelease: pending,
#   autorelease: tagged.
# Any label outside those two sets fails the check.

set -euo pipefail

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
fi

type_labels=(enhancement bug documentation chore refactor test ci release)
aux_labels=(dependencies "autorelease: pending" "autorelease: tagged")

contains() {
  local needle="$1"; shift
  local item
  for item in "$@"; do
    [ "$item" = "$needle" ] && return 0
  done
  return 1
}

type_count=0
unknown=()
while IFS= read -r label; do
  [ -z "$label" ] && continue
  if contains "$label" "${type_labels[@]}"; then
    type_count=$((type_count + 1))
  elif ! contains "$label" "${aux_labels[@]}"; then
    unknown+=("$label")
  fi
done < <(jq -r '.[]')

status=0
if [ "$type_count" -ne 1 ]; then
  echo "::error::PR must carry exactly one type label (one of: ${type_labels[*]}); found ${type_count}."
  status=1
fi
if [ "${#unknown[@]}" -gt 0 ]; then
  echo "::error::PR carries label(s) outside the allowed set: ${unknown[*]}"
  status=1
fi
if [ "$status" -eq 0 ]; then
  echo "PR label policy satisfied: exactly one type label, no stray labels."
fi
exit "$status"
