@microvm
Feature: Docker-compat run/exec flags take effect against a real microVM
  These scenarios boot a real guest via `make e2e-microvm` and drive the
  Docker-style flags this repo added end-to-end: --hostname and -u/--user are
  guest-observable (the code that applies them is platform-only, so Layer 2/3 can
  only fake it), --rm is host-observable after the workload exits, and `lns exec`
  without an explicit `--` exercises the argv normalizer through the real binary.
  Every guest command runs through the bundled busybox by full path.

  Scenario: --hostname sets the hostname the workload observes
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/.lens/guest-tools/bin/busybox hostname" with hostname "demo-box"
    Then the exit code is 0
    And the output contains "demo-box"

  Scenario: -u/--user runs the workload as the requested uid and gid
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/.lens/guest-tools/bin/busybox id" as user "1000:1001"
    Then the exit code is 0
    And the output contains "uid=1000"
    And the output contains "gid=1001"

  Scenario: --rm removes the run record once the workload exits
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/.lens/guest-tools/bin/busybox true" with auto-remove
    Then the exit code is 0
    And that run is no longer listed

  Scenario: --rm on a detached run removes the record after it exits
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/.lens/guest-tools/bin/busybox true" with auto-remove
    Then the exit code is 0
    And that run is no longer listed

  Scenario: --mount keyed bind syntax exposes host files, and readonly rejects writes
    Given the Lens Sandbox service is running
    And a host directory with a file "greeting" containing "keyed-host"
    When the user runs a microVM command "/bin/sh -c 'read v < /work/greeting; echo got=$v; echo back > /work/created'" with a keyed bind at "/work"
    Then the exit code is 0
    And the output contains "got=keyed-host"
    And the host bind directory has a file "created" containing "back"

  Scenario: --mount keyed readonly bind rejects writes
    Given the Lens Sandbox service is running
    And a host directory with a file "secret" containing "ro-keyed"
    When the user runs a microVM command "/bin/sh -c 'read v < /work/secret; echo read=$v; if echo x > /work/blocked 2>/dev/null; then echo WROTE; else echo BLOCKED; fi'" with a read-only keyed bind at "/work"
    Then the exit code is 0
    And the output contains "read=ro-keyed"
    And the output contains "BLOCKED"

  Scenario: --mount keyed volume syntax mounts a writable named volume
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox touch /cache/ok && echo vol-ok'" with keyed volume "compat-keyed-cache" at "/cache"
    Then the exit code is 0
    And the output contains "vol-ok"

  Scenario: the -it cluster expands and the workload runs
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/.lens/guest-tools/bin/busybox echo it-ok" with the -it cluster
    Then the exit code is 0
    And the output contains "it-ok"

  Scenario: exec runs a command in a running sandbox without an explicit separator
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/.lens/guest-tools/bin/busybox sleep 60"
    Then the exit code is 0
    When the user execs "/.lens/guest-tools/bin/busybox echo exec-ok" into that run without a separator
    Then the exit code is 0
    And the output contains "exec-ok"
