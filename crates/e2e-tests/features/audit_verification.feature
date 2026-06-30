Feature: lns audit inspects and verifies audit logs and the connection ledger
  `lns audit <run-id>` shows a run's audit timeline and `lns audit verify
  <run-id>` confirms its chain hasn't been tampered with. `lns audit log`
  and `lns audit connections` read the global connection ledger, and `lns
  audit verify` (no run id) checks it. An intact chain verifies cleanly; a
  missing log or a broken chain exits non-zero with a diagnostic.

  Scenario: a bare run id shows the run's audit timeline
    Given a clean lns cache home
    And a run "smoke" with a valid audit chain
    When I run "lns audit smoke"
    Then the exit code is 0
    And the output contains "audit_event"

  Scenario: an intact chain verifies cleanly
    Given a clean lns cache home
    And a run "smoke" with a valid audit chain
    When I run "lns audit verify smoke"
    Then the exit code is 0
    And the output contains "Verified"

  Scenario: verifying a missing run surfaces a clear error
    Given a clean lns cache home
    When I run "lns audit verify nonexistent"
    Then the exit code is non-zero
    And the output contains "audit log"

  Scenario: a tampered chain is detected and named
    Given a clean lns cache home
    And a run "tamper" with a tampered audit chain
    When I run "lns audit verify tamper"
    Then the exit code is non-zero
    And the output contains "TAMPERED"
    And the output contains "prev_hash"

  Scenario: stdout closing mid-write does not panic
    Given a clean lns cache home
    And a run "pipeable" with a valid audit chain
    When I run "lns audit verify pipeable" with stdout closed
    Then the exit code is non-zero
    And the output does not contain "panicked"

  Scenario: the connection ledger timeline lists recorded events
    Given a clean lns cache home
    And a connection ledger with sample events
    When I run "lns audit log"
    Then the exit code is 0
    And the output contains "some-oauth"
    And the output contains "some-provider"

  Scenario: connections are summarized by integration
    Given a clean lns cache home
    And a connection ledger with sample events
    When I run "lns audit connections"
    Then the exit code is 0
    And the output contains "some-oauth"
    And the output contains "@some-user"
    And the output contains "some-provider"

  Scenario: the connection ledger verifies cleanly
    Given a clean lns cache home
    And a connection ledger with sample events
    When I run "lns audit verify"
    Then the exit code is 0
    And the output contains "Verified"

  Scenario: reading a tampered ledger still lists events but warns about integrity
    Given a clean lns cache home
    And a connection ledger with a tampered event
    When I run "lns audit log"
    Then the exit code is 0
    And the output contains "integrity"
    And the output contains "some-oauth"
