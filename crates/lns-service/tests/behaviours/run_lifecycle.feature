Feature: run lifecycle — graceful stop over IPC
  `lns sandbox stop` asks the service to end a run by sending SIGTERM,
  waiting for the workload to exit, and escalating to SIGKILL at the
  deadline. These scenarios pin the response contract of the platform-
  neutral paths; the signal escalation itself is pinned by technical
  units next to the dispatcher.

  Scenario: Stopping an unknown run surfaces an Error
    Given a fresh service handler
    When a StopRun request for run 99999 arrives
    Then the response is Error
    And the error message contains "no such run: 99999"

  Scenario: Stopping a run that has already exited succeeds without escalation
    Given a registered run that has already exited
    When a StopRun request for that run arrives
    Then the response is RunStopped without force

  Scenario: Inspecting an unknown run answers a typed RunUnknown
    Given a fresh service handler
    When an InspectRun request for run 99999 arrives
    Then the response is RunUnknown for run "99999"

  Scenario: Inspecting a registered run reports its state and launch configuration
    Given a registered run launched from "some-image:1" with 2 cpus and 1024 MiB
    When an InspectRun request for that run arrives
    Then the inspect details name image "some-image:1"
    And the inspect details report 2 cpus and 1024 MiB
    And the inspect details report the run as running

  Scenario: Inspecting a stopped run reports the size it booted with
    Given a stopped run that asked for 1 cpu and 512 MiB and booted with 6 cpus and 4096 MiB
    When an InspectRun request for that run arrives
    Then the inspect details report 6 cpus and 4096 MiB

  Scenario: Removing an unknown run surfaces an Error
    Given a fresh service handler
    When a RemoveRun request for run 99999 arrives
    Then the response is Error
    And the error message contains "no such run: 99999"

  Scenario: Removing a still-running run is refused
    Given a registered run that is still running
    When a RemoveRun request for that run arrives
    Then the response is Error
    And the error message contains "still running"

  Scenario: Removing a run that has already exited is acknowledged
    Given a registered run that has already exited
    When a RemoveRun request for that run arrives
    Then the response is Acknowledged
