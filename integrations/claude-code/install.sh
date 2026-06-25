#!/usr/bin/env bash
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"

echo "Building lns-cc (release)…"
cargo build --release --manifest-path "$DIR/Cargo.toml"
mkdir -p "$DIR/bin"
cp "$DIR/target/release/lns-cc" "$DIR/bin/lns-cc"
echo "Built $DIR/bin/lns-cc"
echo

if ! command -v lns >/dev/null 2>&1; then
  echo "WARNING: \`lns\` is not on PATH — install it from https://get.lns.run"
elif ! lns service status >/dev/null 2>&1; then
  echo "NOTE: the Lens Sandbox service is not running — start it with \`lns service start\`"
fi

cat <<EOF

Test it in Claude Code:

  1. Fastest (no install) — launch with the plugin loaded directly:
       claude --plugin-dir "$DIR"
     Run /reload-plugins after any rebuild.

  2. Install as a plugin (persists across sessions):
       /plugin marketplace add "$DIR"
       /plugin install lns-sandbox@lns-plugins
       /reload-plugins

Verify with /plugin (Installed tab) and /help (shows /lns-sandbox), then run any
sandbox-worthy command, e.g. python -c "print('hi')".
EOF
