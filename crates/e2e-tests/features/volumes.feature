@microvm
Feature: named volumes persist workload state across runs
  `lns run` starts from a blank, discarded writable layer every time. A named
  volume is host-backed storage, attached with `-v name:/path[:ro]`, whose
  contents survive between runs. Ephemeral-by-default is preserved: no volume
  is attached unless the user asks for one.

  These are guest-observable and need a booted microVM, so they are @microvm
  and run only via `make e2e-microvm`. They are imageless — the commands use
  /bin/sh builtins (echo, read, redirects) and the bundled busybox by full
  path, so no registry pull is needed. The image-seeding scenarios that need
  a real image live parked in specs/microvm/volume_seeding.feature until
  `lns image import` exists.

  Scenario: data written to a volume is there on the next run
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo persisted-data > /data/marker'" with volume "e2e-vol-persist" at "/data"
    Then the exit code is 0
    And volume "e2e-vol-persist" is released
    When the user runs a microVM command "/bin/sh -c 'read v < /data/marker; echo got=$v'" with volume "e2e-vol-persist" at "/data"
    Then the exit code is 0
    And the output contains "got=persisted-data"

  Scenario: without a volume the run is fully ephemeral
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo ghost > /run/marker; read v < /run/marker; echo wrote=$v'"
    Then the exit code is 0
    And the output contains "wrote=ghost"
    When the user runs a microVM command "/bin/sh -c 'if [ -f /run/marker ]; then echo PRESENT; else echo ABSENT; fi'"
    Then the exit code is 0
    And the output contains "ABSENT"

  Scenario: a volume is name-keyed, not path-keyed
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo keyed-by-name > /data/marker'" with volume "e2e-vol-key" at "/data"
    Then the exit code is 0
    And volume "e2e-vol-key" is released
    When the user runs a microVM command "/bin/sh -c 'read v < /srv/state/marker; echo got=$v'" with volume "e2e-vol-key" at "/srv/state"
    Then the exit code is 0
    And the output contains "got=keyed-by-name"

  Scenario: a read-only attach exposes data but rejects writes
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo seeded-content > /data/marker'" with volume "e2e-vol-ro" at "/data"
    Then the exit code is 0
    And volume "e2e-vol-ro" is released
    When the user runs a microVM command "/bin/sh -c 'read v < /data/marker; echo read=$v; if echo blocked > /data/blocked 2>/dev/null; then echo WROTE; else echo BLOCKED; fi'" with read-only volume "e2e-vol-ro" at "/data"
    Then the exit code is 0
    And the output contains "read=seeded-content"
    And the output contains "BLOCKED"

  Scenario: an invalid volume name is rejected before the VM boots
    Given the Lens Sandbox service is running
    When I run "run -v ../etc:/data -- /bin/true"
    Then the exit code is non-zero
    And the output contains "invalid volume name"

  Scenario: attaching a volume is recorded in the audit history
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo ok'" with volume "e2e-vol-audit" at "/data"
    Then the exit code is 0
    And the audit chain for that run records volume "e2e-vol-audit" at "/data"

  Scenario: the same volume cannot be attached to two live runs at once
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'" with volume "e2e-vol-live" at "/data"
    Then the exit code is 0
    When the user runs a microVM command "/bin/sh -c 'echo got-it'" with volume "e2e-vol-live" at "/data"
    Then the exit code is non-zero
    And the output contains "in use"

  Scenario: releasing a killed workload's volume flushes its unsynced write
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c 'echo survived > /data/marker; echo wrote=$?; /.lens/guest-tools/bin/busybox sleep 60'" with volume "e2e-vol-killed" at "/data"
    Then the exit code is 0
    When the user prints that run's logs until they contain "wrote=0"
    Then the exit code is 0
    When the user kills that run
    Then volume "e2e-vol-killed" is released
    When the user runs a microVM command "/bin/sh -c 'read v < /data/marker; echo got=$v'" with volume "e2e-vol-killed" at "/data"
    Then the exit code is 0
    And the output contains "got=survived"

  Scenario: a volume the run finished with is left marked clean
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo persisted > /data/marker'" with volume "e2e-vol-clean" at "/data"
    Then the exit code is 0
    And volume "e2e-vol-clean" is released
    And the backing image for volume "e2e-vol-clean" is marked clean
