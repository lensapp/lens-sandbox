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

Enable the plugin in Claude Code, either:

  1. As a plugin (recommended) — register this directory; Claude Code reads
     .claude-plugin/plugin.json and hooks/hooks.json automatically:
       $DIR

  2. Quick trial — add to ~/.claude/settings.json:
       "hooks": {
         "PreToolUse": [
           { "matcher": "Bash",
             "hooks": [ { "type": "command", "command": "$DIR/bin/lns-cc hook" } ] }
         ],
         "SessionStart": [
           { "hooks": [ { "type": "command", "command": "$DIR/bin/lns-cc doctor" } ] }
         ]
       }

Then run any sandbox-worthy command (e.g. python -c "print('hi')") in a new session.
EOF
