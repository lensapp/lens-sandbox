#!/usr/bin/env bash
# Verify that SHA256 pins in kernels.toml match bytes at the corresponding CDN URLs.
#
# Usage: scripts/check-kernel-pins.sh [--allow-pending]
#
# --allow-pending: treat CDN 404 as non-fatal (used on bump PRs before the
#   publish-kernel workflow has uploaded the new artifacts to the CDN).
#   SHA mismatches are always fatal regardless of this flag.

set -euo pipefail

ALLOW_PENDING=0
for arg in "$@"; do
  case "$arg" in
    --allow-pending) ALLOW_PENDING=1 ;;
    -h|--help)
      sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown arg: $arg" >&2
      exit 2
      ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$REPO_ROOT/crates/lns-service/kernels.toml"
CDN_BASE="https://get.lns.run"

if [ ! -f "$MANIFEST" ]; then
  echo "ERROR: $MANIFEST not found" >&2
  exit 1
fi

# python3 is the only portable TOML parser available on ubuntu-latest and macOS.
PAIRS=$(python3 - "$MANIFEST" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as f:
    m = tomllib.load(f)
ver = m["current"]["published_version"]
print(f"VERSION {ver}")
for arch, sha in m["current"]["sha256"].items():
    print(f"{arch} {sha}")
PY
)

VERSION=""
EXIT=0
SAW_PENDING=0
SAW_OK=0

while IFS=' ' read -r KEY VALUE; do
  if [ "$KEY" = "VERSION" ]; then
    VERSION="$VALUE"
    echo "== Verifying kernels.toml against $CDN_BASE (published_version=$VERSION)"
    continue
  fi

  ARCH="$KEY"
  EXPECTED="$VALUE"
  URL="$CDN_BASE/lns-kernel-${VERSION}-${ARCH}"

  if [ -z "$EXPECTED" ]; then
    if [ "$ALLOW_PENDING" -eq 1 ]; then
      echo "   $ARCH: sha is empty in manifest (pending bot back-fill) — allowed"
      SAW_PENDING=$((SAW_PENDING + 1))
      continue
    fi
    echo "   $ARCH: sha is empty in manifest" >&2
    EXIT=1
    continue
  fi

  # HEAD first so a 404 is distinguishable from a sha mismatch without downloading.
  STATUS=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 30 --head "$URL" || echo "000")

  if [ "$STATUS" = "404" ]; then
    if [ "$ALLOW_PENDING" -eq 1 ]; then
      echo "   $ARCH: CDN 404 at $URL — pending publish (allowed)"
      SAW_PENDING=$((SAW_PENDING + 1))
      continue
    fi
    echo "   $ARCH: CDN 404 at $URL (manifest pins this URL but it's not published)" >&2
    EXIT=1
    continue
  fi

  if [ "$STATUS" != "200" ]; then
    echo "   $ARCH: CDN HEAD returned $STATUS for $URL" >&2
    EXIT=1
    continue
  fi

  ACTUAL=$(curl -sS --fail --max-time 120 "$URL" | shasum -a 256 | awk '{print $1}')
  if [ "$ACTUAL" = "$EXPECTED" ]; then
    echo "   $ARCH: OK ($ACTUAL)"
    SAW_OK=$((SAW_OK + 1))
  else
    echo "   $ARCH: SHA MISMATCH at $URL" >&2
    echo "          expected $EXPECTED" >&2
    echo "          got      $ACTUAL" >&2
    EXIT=1
  fi
done <<<"$PAIRS"

echo "== Summary: verified=$SAW_OK pending=$SAW_PENDING failed=$([ $EXIT -eq 0 ] && echo 0 || echo 1)"
exit "$EXIT"
