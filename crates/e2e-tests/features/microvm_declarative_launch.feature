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
