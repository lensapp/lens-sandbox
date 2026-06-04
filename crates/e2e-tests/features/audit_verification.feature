Feature: lns audit verifies the audit chain
  Users who want to confirm that a completed run's audit log hasn't
  been tampered with can run `lns audit <run-id>`. An intact chain
  exits cleanly; a missing log or a broken chain exits non-zero with
  a diagnostic.

  Scenario: an intact chain verifies cleanly
    Given a clean lns cache home
    And a run "smoke" with a valid audit chain
    When I run "lns audit smoke"
    Then the exit code is 0
    And the output contains "Verified"

  Scenario: a missing run-id surfaces a clear error
    Given a clean lns cache home
    When I run "lns audit nonexistent"
    Then the exit code is non-zero
    And the output contains "audit log"

  Scenario: a tampered chain is detected and named
    Given a clean lns cache home
    And a run "tamper" with a tampered audit chain
    When I run "lns audit tamper"
    Then the exit code is non-zero
    And the output contains "TAMPERED"
    And the output contains "prev_hash"

  Scenario: stdout closing mid-write does not panic
    Given a clean lns cache home
    And a run "pipeable" with a valid audit chain
    When I run "lns audit pipeable" with stdout closed
    Then the exit code is non-zero
    And the output does not contain "panicked"
