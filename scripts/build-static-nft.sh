#!/usr/bin/env bash
# Build a statically-linked nft binary via docker buildx and write it to vendor/static-nft/.
#
# Usage:
#   scripts/build-static-nft.sh                 # arm64 (default)
#   scripts/build-static-nft.sh linux/arm64
#   scripts/build-static-nft.sh linux/amd64

set -euo pipefail

# Pinned in lockstep with upstream's Dockerfile — bump both together to stay on their tested combination.
NFTABLES_VERSION=1.1.5
NFTABLES_SHA256=1daf10f322e14fd90a017538aaf2c034d7cc1eb1cc418ded47445d714ea168d4

PLATFORM="${1:-linux/arm64}"
case "$PLATFORM" in
  linux/arm64) ARCH=arm64 ;;
  linux/amd64) ARCH=amd64 ;;
  *) echo "unsupported platform: $PLATFORM (want linux/arm64 or linux/amd64)" >&2; exit 1 ;;
esac

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$REPO_ROOT/vendor/static-nft"
STAGE_DIR="$(mktemp -d -t lns-static-nft.XXXXXX)"
trap 'rm -rf "$STAGE_DIR"' EXIT

mkdir -p "$OUT_DIR"

ASSET="nft-${NFTABLES_VERSION}-linux-${ARCH}-musl"

echo "Building $ASSET via docker buildx (platform=$PLATFORM)..."
docker buildx build \
  --platform "$PLATFORM" \
  --build-arg "NFTABLES_VERSION=$NFTABLES_VERSION" \
  --build-arg "NFTABLES_SHA256=$NFTABLES_SHA256" \
  --output "type=local,dest=$STAGE_DIR" \
  -f "$REPO_ROOT/scripts/static-nft.Dockerfile" \
  "$REPO_ROOT/scripts"

if [ ! -f "$STAGE_DIR/nft" ]; then
  echo "build succeeded but $STAGE_DIR/nft missing — Dockerfile layout drift?" >&2
  exit 1
fi

# A real static nft binary is well over 1 MB; anything smaller is a broken build.
SIZE=$(stat -f%z "$STAGE_DIR/nft" 2>/dev/null || stat -c%s "$STAGE_DIR/nft")
if [ "$SIZE" -lt 500000 ]; then
  echo "built nft is suspiciously small ($SIZE bytes) — refusing to publish" >&2
  exit 1
fi

# Atomic install to avoid a half-written binary if interrupted.
TMP="$OUT_DIR/.${ASSET}.tmp.$$"
cp "$STAGE_DIR/nft" "$TMP"
chmod 0755 "$TMP"
mv "$TMP" "$OUT_DIR/$ASSET"

( cd "$OUT_DIR" && shasum -a 256 "$ASSET" > "$ASSET.sha256" )

echo
echo "Wrote:"
ls -la "$OUT_DIR/$ASSET" "$OUT_DIR/$ASSET.sha256"
cat "$OUT_DIR/$ASSET.sha256"
