Feature: lns start — restart a stopped run from the CLI
  `lns start <RUN>` is the top-level shortcut for `lns sandbox start`.
  Detached by default: it prints the run's handle and exits; -a attaches
  output and adopts the workload's exit code.

  Scenario: starting a stopped run detached
    Given a run named "reviewer" that was stopped
    When I run "lns start reviewer"
    Then it prints "reviewer"
    And it exits 0
    And the run "reviewer" is running

  Scenario: start accepts the numeric id
    Given a stopped run with id 7
    When I run "lns start 7"
    Then it exits 0

  Scenario: -a attaches and propagates the workload exit code
    Given a stopped run whose workload exits with code 3 after restart
    When I run "lns start -a" on it
    Then I see the workload output
    And it exits 3