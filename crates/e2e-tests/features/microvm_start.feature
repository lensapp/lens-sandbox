Feature: lns start — the writable layer across stop and start
  A stopped run's writable layer survives on disk; start boots the same
  run back on top of it. Runtime state — processes and tmpfs — never
  survives.

  @microvm
  Scenario: the writable layer survives stop and start
    Given a run in which a file was written outside any mount
    And the run was stopped
    When I start it
    Then the file is present with its contents

  @microvm
  Scenario: processes and tmpfs do not survive
    Given a stopped run whose workload had written to /tmp
    When I start it
    Then the workload's entrypoint runs again from the start
    And /tmp is empty
