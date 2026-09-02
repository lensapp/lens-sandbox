@microvm
Feature: -w sets the workload's working directory in the guest
  `-w` chooses the directory the workload starts in (created if missing).
  This is guest-observable: the workload reports its own working directory
  from inside the booted guest. The marker is shell-computed so it matches
  only real output, not the echoed command line.

  Scenario: a workdir is the guest working directory at exec
    Given the LNS service is running
    When the user runs a microVM command "/bin/sh -c 'echo wd=$(/.lens/guest-tools/bin/busybox pwd)'" with workdir "/tmp"
    Then the exit code is 0
    And the output contains "wd=/tmp"
