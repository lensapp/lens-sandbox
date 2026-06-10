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
    And the error message contains "no active run with id 99999"

  Scenario: Stopping a run that has already exited succeeds without escalation
    Given a registered run that has already exited
    When a StopRun request for that run arrives
    Then the response is RunStopped without force
