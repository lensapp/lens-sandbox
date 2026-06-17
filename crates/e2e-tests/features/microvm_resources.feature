@microvm
Feature: resource limits are honored inside the guest
  `--cpus` sizes the guest's virtual hardware. This is guest-observable: the
  workload reads its processor count back from inside the booted guest. The
  marker is computed by the shell (not a literal in the command) so it can
  only match real workload output, never the echoed command line.

  Scenario: --cpus sets the processor count the guest sees
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo nproc=$(/.lens/guest-tools/bin/busybox nproc)'" with 2 vCPUs
    Then the exit code is 0
    And the output contains "nproc=2"
