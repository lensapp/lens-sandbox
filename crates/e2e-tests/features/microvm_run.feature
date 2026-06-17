@microvm
Feature: an imageless command runs to completion inside a real microVM
  These scenarios boot a real guest through the Vz backend on macOS. They are
  excluded from cargo test and regular CI (no virtualization there) and run
  only via `make e2e-microvm` on a virt-capable host, where they pin the live
  boot -> lns-init -> broker -> workload -> exit-code path that the Layer 2/3
  tests can only fake. An imageless run pulls no registry image: the guest
  boots off the pinned kernel and the bundled busybox, so /bin/sh is present.

  Scenario: an imageless run boots a guest, runs a command, and relays its stdout
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo hello-from-guest'"
    Then the exit code is 0
    And the output contains "hello-from-guest"

  Scenario: a non-zero guest exit status propagates to the host
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'exit 3'"
    Then the exit code is 3
