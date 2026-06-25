---
description: Manage the Lens Sandbox plugin — show status, grant a host path, or clean up sandboxes
argument-hint: "[status | grant <path> [--rw] | clean]"
allowed-tools: Bash
---

The Lens Sandbox plugin routes the agent's code-execution commands into `lns` microVMs.
Use its admin binary at `${CLAUDE_PLUGIN_ROOT}/bin/lns-cc` to handle this request: `$ARGUMENTS`

Pick the matching action and run exactly one command, then summarize its output:

- empty or `status` → `${CLAUDE_PLUGIN_ROOT}/bin/lns-cc status`
  (effective config, lns/service health, active `cc-*` sandboxes)
- `grant <path>` (optionally `--rw`) → `${CLAUDE_PLUGIN_ROOT}/bin/lns-cc grant <path> [--rw]`
  (exposes a host path to the sandbox, read-only by default; refuses secret paths)
- `clean` → `${CLAUDE_PLUGIN_ROOT}/bin/lns-cc clean`
  (removes the plugin's leftover `cc-*` sandboxes)

If the argument doesn't match any of these, show the usage line above instead of guessing.
