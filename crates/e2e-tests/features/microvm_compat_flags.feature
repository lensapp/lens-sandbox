@microvm
Feature: Docker-compat run/exec flags take effect against a real microVM
  These imageless scenarios boot a real guest via `make e2e-microvm` and drive the
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

  Scenario: exec runs a command in a running sandbox without an explicit separator
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/.lens/guest-tools/bin/busybox sleep 60"
    Then the exit code is 0
    When the user execs "/.lens/guest-tools/bin/busybox echo exec-ok" into that run without a separator
    Then the exit code is 0
    And the output contains "exec-ok"
