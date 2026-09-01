@microvm
Feature: -w sets the workload's working directory in the guest
  `-w` chooses the directory the workload starts in (created if missing).
  This is guest-observable: the workload reports its own working directory
  from inside the booted guest. The marker is shell-computed so it matches
  only real output, not the echoed command line.

  Scenario: a workdir is the guest working directory at exec
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo wd=$(/.lens/guest-tools/bin/busybox pwd)'" with workdir "/tmp"
    Then the exit code is 0
    And the output contains "wd=/tmp"

  Scenario: a workdir the guest has to create belongs to the run-as user
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo owner=$(/.lens/guest-tools/bin/busybox stat -c %u:%g /work/inner)'" with workdir "/work/inner"
    Then the exit code is 0
    And the output contains "owner=65534:65534"

  Scenario: a workdir the image already ships keeps its own ownership
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo owner=$(/.lens/guest-tools/bin/busybox stat -c %u /tmp)'" with workdir "/tmp"
    Then the exit code is 0
    And the output contains "owner=0"
