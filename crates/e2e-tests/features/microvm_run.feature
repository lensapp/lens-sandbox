@microvm
Feature: a command runs to completion inside a real microVM
  These scenarios boot a real guest through the Vz backend on macOS. They are
  excluded from cargo test and regular CI (no virtualization there) and run
  only via `make e2e-microvm` on a virt-capable host, where they pin the live
  boot -> lns-init -> broker -> workload -> exit-code path that the Layer 2/3
  tests can only fake. Every run boots the standard alpine base image
  (imageless mode is retired — an omitted REF now means ./lns.yaml).

  Scenario: a run boots a guest, runs a command, and relays its stdout
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo relayed-$((21*2))'"
    Then the exit code is 0
    And the output contains "relayed-42"

  Scenario: a non-zero guest exit status propagates to the host
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'exit 3'"
    Then the exit code is 3

  Scenario: a large multi-line stream relays in full, first and last line intact
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'i=0; while [ $i -lt 2000 ]; do echo line-$i; i=$((i+1)); done'"
    Then the exit code is 0
    And the output contains "line-0"
    And the output contains "line-1999"

  Scenario: both stdout and stderr from the guest reach the host
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo out-$((6*7)); echo err-$((8*9)) >&2'"
    Then the exit code is 0
    And the output contains "out-42"
    And the output contains "err-72"

  Scenario: output without a trailing newline is not dropped
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'printf tail-$((100+11))'"
    Then the exit code is 0
    And the output contains "tail-111"

  Scenario: a command that writes then exits at once does not lose its final line
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo final-$((9*9)); exit 0'"
    Then the exit code is 0
    And the output contains "final-81"
