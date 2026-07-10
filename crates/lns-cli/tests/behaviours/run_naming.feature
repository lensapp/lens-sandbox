Feature: addressing runs by name from the CLI
  `lns run --name` gives a run a human handle, and every `lns sandbox`
  verb takes that name wherever it takes a numeric run id. The CLI stays
  thin: it forwards whatever handle the user typed and lets the service
  resolve it, so numeric-id addressing keeps working unchanged.

  Scenario: a lifecycle verb forwards a name as the run handle
    Given the service will answer RunStopped without force
    When the user runs sandbox command "stop reviewer"
    Then the exit code is 0
    And the output contains "stopped run reviewer"
    And the service received a StopRun request for run "reviewer" with timeout 10

  Scenario: numeric-id addressing is unchanged
    Given the service will answer RunStopped without force
    When the user runs sandbox command "stop 3"
    Then the exit code is 0
    And the output contains "stopped run 3"
    And the service received a StopRun request for run "3" with timeout 10

  Scenario: the run list shows a NAME column with each run's name
    Given the service reports a run listing with run 3 named "reviewer" of image "some-image" running
    When the user runs sandbox command "ls"
    Then the exit code is 0
    And the output contains "NAME"
    And the output contains "reviewer"

  Scenario: a naming error from the service is surfaced verbatim
    Given the service will answer an error "no active run with id reviewer"
    When the user runs sandbox command "stop reviewer"
    Then the command fails with an exit code other than 0
    And the output contains "no active run with id reviewer"
