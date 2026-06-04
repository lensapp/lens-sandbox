#!/usr/bin/env bash
# Build, codesign, start the service, and exec lns — the single dev-iteration command.
#
# Usage: ./scripts/lns-cli.sh <any lns args>
#
# LNS_DEV_PROFILE  cargo profile: "debug" (default) or "release"
#
# When iterating on lns-service, run `./scripts/lns-cli.sh service stop` first
# so the rebuilt binary gets exec'd rather than the already-running one.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${LNS_DEV_PROFILE:-debug}"
USER_PWD="$PWD"

cd "$REPO_ROOT"

# Propagates the inner exit code (not just 1) so callers can distinguish compile vs. codesign errors.
run_quiet() {
  local out rc
  out=$("$@" 2>&1)
  rc=$?
  if (( rc != 0 )); then
    printf '%s\n' "$out" >&2
    exit "$rc"
  fi
}

case "$PROFILE" in
  debug)
    run_quiet cargo build --quiet -p lns-cli -p lns-service
    BIN="$REPO_ROOT/target/debug/lns"
    SERVICE_BIN="$REPO_ROOT/target/debug/lns-service"
    ;;
  release)
    run_quiet cargo build --quiet --release -p lns-cli -p lns-service
    BIN="$REPO_ROOT/target/release/lns"
    SERVICE_BIN="$REPO_ROOT/target/release/lns-service"
    ;;
  *)
    echo "Unsupported LNS_DEV_PROFILE: $PROFILE (use 'debug' or 'release')" >&2
    exit 1
    ;;
esac

# macOS Vz invalidates the codesign on every rebuild; re-sign so `lns run` doesn't fail with NSCocoaErrorDomain 4097.
run_quiet "$REPO_ROOT/crates/lns-cli/scripts/codesign-macos.sh" "$BIN"
run_quiet "$REPO_ROOT/crates/lns-cli/scripts/codesign-macos.sh" "$SERVICE_BIN"

# Override PATH lookup so `service start` and `require_running` spawn this dev build.
export LNS_SERVICE_BIN="$SERVICE_BIN"

# `lns service start` is idempotent (pings first), so call unconditionally.
# Skip for `service` subcommands so explicit stop/status reflects real state.
case "${1:-}" in
  service) ;;
  *) run_quiet "$BIN" service start ;;
esac

# Restore the caller's cwd so relative paths in "$@" resolve correctly.
cd "$USER_PWD"

exec "$BIN" "$@"
