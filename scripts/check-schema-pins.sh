#!/usr/bin/env bash
# Verify the vendored lens-artifact-spec schemas match the pinned upstream commit.
#
# Usage: scripts/check-schema-pins.sh
#
# Reads crates/lns-service/src/artifact/schemas/PINNED (repo on line 1, commit
# on line 2) and byte-compares every vendored *.v1alpha1.json against the same
# path at that commit. The spec repo is private, so this uses authenticated
# `gh api` — set GH_TOKEN to a token with read access. Any drift is fatal: the
# vendored copy is the offline build contract and must not diverge from the pin.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$REPO_ROOT/crates/lns-service/src/artifact/schemas"
PINNED="$DIR/PINNED"

if [ ! -f "$PINNED" ]; then
  echo "ERROR: $PINNED not found" >&2
  exit 1
fi
if ! command -v gh >/dev/null 2>&1; then
  echo "ERROR: gh CLI required — the spec repo is private and needs authenticated access" >&2
  exit 1
fi

REPO="$(sed -n '1p' "$PINNED")"
COMMIT="$(sed -n '2p' "$PINNED")"

if [ -z "$REPO" ] || [ -z "$COMMIT" ]; then
  echo "ERROR: $PINNED must carry the repo on line 1 and the commit sha on line 2" >&2
  exit 1
fi

echo "== Verifying vendored schemas against $REPO @ $COMMIT (via gh api)"

EXIT=0
SAW_OK=0
for file in "$DIR"/*.v1alpha1.json; do
  name="$(basename "$file")"
  tmp="$(mktemp)"
  if ! gh api "repos/$REPO/contents/schemas/$name?ref=$COMMIT" --jq '.content' 2>/dev/null \
    | base64 -d > "$tmp" 2>/dev/null; then
    echo "   $name: could not fetch from $REPO @ $COMMIT" >&2
    EXIT=1
    rm -f "$tmp"
    continue
  fi
  if diff -q "$tmp" "$file" >/dev/null; then
    echo "   $name: OK"
    SAW_OK=$((SAW_OK + 1))
  else
    echo "   $name: DRIFT from $REPO @ $COMMIT" >&2
    EXIT=1
  fi
  rm -f "$tmp"
done

echo "== Summary: verified=$SAW_OK failed=$([ $EXIT -eq 0 ] && echo 0 || echo 1)"
exit "$EXIT"
