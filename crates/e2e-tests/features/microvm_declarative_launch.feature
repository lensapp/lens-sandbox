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

  Scenario: a pulled published sandbox launches offline from the consumer project
    Given a local registry
    And the Lens Sandbox service is running
    When the user pulls a published declarative sandbox and runs it with the registry offline from a consumer project
    Then the exit code is 0
    And the output contains "wd=/workspace"
    And the output contains "consumer=visible"
    And the output contains "bind=readonly"
    And the output contains "volume=mounted"
