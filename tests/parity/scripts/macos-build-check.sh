#!/usr/bin/env bash
# Check that a backend's two binaries are macOS builds this host can run.
# Usage: macos-build-check.sh <lns> <lns-service>
set -euo pipefail

[ "$#" -eq 2 ] || { echo "usage: $0 <lns> <lns-service>" >&2; exit 2; }
[ "$(uname -s)" = "Darwin" ] || { echo "[skip]  this host is $(uname -s), not Darwin"; exit 0; }

for binary in "$@"; do
  [ -x "$binary" ] || { echo "[fail]  $binary is not executable" >&2; exit 1; }
  file "$binary" | grep -q Mach-O || { echo "[fail]  $binary is not a Mach-O binary" >&2; exit 1; }
  codesign --verify --verbose "$binary" 2>&1 | tail -1
  echo "[ok]    $binary"
done
