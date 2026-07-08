@microvm
Feature: Docker-compat run flags change what the workload sees inside a real microVM
  These imageless scenarios boot a real guest via `make e2e-microvm` and assert,
  from inside the guest, that --hostname and -u/--user take effect — the
  guest-side behaviour that the Layer 2/3 tests can only fake because the code
  that applies it (unshare + sethostname, the uid/gid drop) is platform-only.
  Every command runs through the bundled busybox by full path.

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
