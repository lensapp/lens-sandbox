@microvm
Feature: a pre-start script prepares a real guest without holding the boot
  A `pre-start` script runs in the booted guest before the workload
  (`docs/sandbox-spec.md` §3.1.13). What a script may read is what this covers:
  its stdin is `/dev/null`, so a tool that would ask a question reads an EOF and
  the run reaches its workload. An inherited stdin has no one answering it before
  the workload starts, and no timeout bounds a script, so a read there would hold
  the boot open for good — with the script's own output the only clue, which a
  quiet install does not give.

  Scenario: a script that reads its stdin does not hold the boot
    Given the LNS service is running
    And the project definition declares a pre-start script "cat > /dev/null"
    And the project definition sets command "/bin/sh -c '/.lens/guest-tools/bin/busybox echo the workload ran'"
    When the user runs the sandbox definition
    Then the exit code is 0
    And the output contains "the workload ran"
