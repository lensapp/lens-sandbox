#!/usr/bin/env bash
# Re-derive the embedded static nft binary from sha-pinned source and prove the
# committed vendor/static-nft/ blob is byte-identical to a fresh build.
#
# This is the independent re-derivation the size-floor + build.rs sha pin can't
# give on their own: it stops a PR that swaps the vendored blob *and* the pinned
# sha256 in build.rs from passing review-blind, because the rebuilt bytes come
# from scripts/static-nft.Dockerfile (which pins NFTABLES_SHA256), not the PR.
#
# Usage:
#   scripts/verify-static-nft.sh linux/arm64
#   scripts/verify-static-nft.sh linux/amd64

set -euo pipefail

PLATFORM="${1:-linux/arm64}"
case "$PLATFORM" in
  linux/arm64) ARCH=arm64 ;;
  linux/amd64) ARCH=amd64 ;;
  *) echo "unsupported platform: $PLATFORM (want linux/arm64 or linux/amd64)" >&2; exit 1 ;;
esac

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ASSET=$(grep -oE 'NFTABLES_VERSION=[0-9.]+' "$REPO_ROOT/scripts/build-static-nft.sh" | head -1 | cut -d= -f2)
ASSET="nft-${ASSET}-linux-${ARCH}-musl"
COMMITTED="$REPO_ROOT/vendor/static-nft/$ASSET"

if [ ! -f "$COMMITTED" ]; then
  echo "::error::committed blob $COMMITTED is missing — nothing to verify against." >&2
  exit 1
fi

REBUILD_DIR="$(mktemp -d -t lns-verify-nft.XXXXXX)"
trap 'rm -rf "$REBUILD_DIR"' EXIT

LNS_STATIC_NFT_OUT_DIR="$REBUILD_DIR" "$REPO_ROOT/scripts/build-static-nft.sh" "$PLATFORM"

REBUILT="$REBUILD_DIR/$ASSET"
if cmp -s "$REBUILT" "$COMMITTED"; then
  echo "OK: $ASSET re-derived from pinned source is byte-identical to the committed blob."
  shasum -a 256 "$COMMITTED"
  exit 0
fi

echo "::error::$ASSET MISMATCH — the committed blob is NOT what scripts/static-nft.Dockerfile produces." >&2
echo "::error::  rebuilt:   $(shasum -a 256 "$REBUILT"  | awk '{print $1}')" >&2
echo "::error::  committed: $(shasum -a 256 "$COMMITTED" | awk '{print $1}')" >&2
echo "::error::A blob swap requires the same change to land in scripts/static-nft.Dockerfile's pins, in the open." >&2
exit 1
