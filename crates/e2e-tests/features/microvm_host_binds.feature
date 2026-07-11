@microvm
Feature: host bind mounts expose live host files inside the guest
  `lns run -v /host/path:/guest[:ro]` shares a host directory into the workload
  over virtio-fs — distinct from a named volume, which is a block device. The
  host files are visible in the guest, writes to a writable bind land back on
  the host, and a read-only bind rejects writes. These outcomes are guest-
  observable and need a booted microVM, so they are @microvm and run only via
  `make e2e-microvm`. The commands use /bin/sh builtins atop the standard
  alpine base.

  Scenario: a writable host bind exposes host files and writes land back on the host
    Given the Lens Sandbox service is running
    And a host directory with a file "greeting" containing "from-host"
    When the user runs a microVM command "/bin/sh -c 'read v < /work/greeting; echo got=$v; echo from-guest > /work/created'" with a host bind at "/work"
    Then the exit code is 0
    And the output contains "got=from-host"
    And the host bind directory has a file "created" containing "from-guest"

  Scenario: a read-only host bind exposes data but rejects writes
    Given the Lens Sandbox service is running
    And a host directory with a file "secret" containing "ro-data"
    When the user runs a microVM command "/bin/sh -c 'read v < /work/secret; echo read=$v; if echo blocked > /work/blocked 2>/dev/null; then echo WROTE; else echo BLOCKED; fi'" with a read-only host bind at "/work"
    Then the exit code is 0
    And the output contains "read=ro-data"
    And the output contains "BLOCKED"
