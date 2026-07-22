@microvm
Feature: declarative workdir and mounts reach a real guest
  A local lns.yaml can make the project directory the workload's working tree
  and attach persistent storage without repeating launch flags.

  Scenario: a declarative project bind, named volume, and workdir are guest-visible
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo wd=$(/.lens/guest-tools/bin/busybox pwd); if [ -f lns.yaml ]; then echo project=visible; fi; if echo blocked > lns.yaml 2>/dev/null; then echo bind=writable; else echo bind=readonly; fi; echo volume=mounted > /data/marker; read v < /data/marker; echo $v'" from a declarative sandbox
    Then the exit code is 0
    And the output contains "wd=/workspace"
    And the output contains "project=visible"
    And the output contains "bind=readonly"
    And the output contains "volume=mounted"

  Scenario: a path reference launches the definition rooted at its own directory
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo wd=$(/.lens/guest-tools/bin/busybox pwd); if [ -f lns.yaml ]; then echo project=visible; fi'" on the declarative sandbox by path from a nested directory
    Then the exit code is 0
    And the output contains "wd=/workspace"
    And the output contains "project=visible"

  Scenario: a declared path fileset is snapshotted into the guest
    Given the Lens Sandbox service is running
    And the project declares a fileset directory "skills" containing "prompts.md" mounted at "/opt/agent-skills"
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox cat /opt/agent-skills/prompts.md'"
    Then the exit code is 0
    And the output contains "fileset payload"
    And the run output carries no signature warning

  Scenario: a declared fileset transfers to the workload user by default
    Given the Lens Sandbox service is running
    And the project declares a fileset directory "seed" containing "state.json" mounted at "/home/sandbox"
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox stat -c owner=%u /home/sandbox/state.json && echo overwrite > /home/sandbox/state.json && echo writable=yes'"
    Then the exit code is 0
    And the output contains "owner=65534"
    And the output contains "writable=yes"

  Scenario: an owner root fileset stays pinned beyond the workload's reach
    Given the Lens Sandbox service is running
    And the project declares a fileset directory "skills" containing "prompts.md" mounted at "/opt/agent-skills" owned by root
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox stat -c owner=%u /opt/agent-skills/prompts.md; echo tamper > /opt/agent-skills/prompts.md 2>/dev/null || echo denied=yes'"
    Then the exit code is 0
    And the output contains "owner=0"
    And the output contains "denied=yes"

  Scenario: a declared inline fileset is materialized with exact content and workload ownership
    Given the Lens Sandbox service is running
    And the project declares inline file ".claude/settings.json" with content `{"inline":true}` mounted at "/home/sandbox"
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox cat /home/sandbox/.claude/settings.json; /.lens/guest-tools/bin/busybox stat -c owner=%u /home/sandbox/.claude/settings.json'"
    Then the exit code is 0
    And the output contains `{"inline":true}`
    And the output contains "owner=65534"

  Scenario: a root-owned inline fileset cannot be rewritten by the workload
    Given the Lens Sandbox service is running
    And the project declares root-owned inline file "mcp.json" mounted at "/etc/agent"
    When the user runs a microVM command "/bin/sh -c 'echo tamper > /etc/agent/mcp.json 2>/dev/null || echo denied=yes'"
    Then the exit code is 0
    And the output contains "denied=yes"

  Scenario: a pulled published sandbox launches offline from the consumer project
    Given a local registry
    And the Lens Sandbox service is running
    When the user pulls a published declarative sandbox and runs it with --yes with the registry offline from a consumer project
    Then the exit code is 0
    And the output does not contain "mounts into the workload"
    And the output does not contain "Continue?"
    And the output contains "wd=/workspace"
    And the output contains "consumer=visible"
    And the output contains "bind=readonly"
    And the output contains "volume=mounted"
