Feature: lns audit reads OCSF-format logs identically to legacy
  New audit lines are written in OCSF v1.7.0; `lns audit` reconstructs
  the same human timeline it shows for legacy lines, and a single log
  file may hold both formats during the forward-only migration.

  Scenario: an OCSF guest egress event reads like the approval prompt
    Given a clean lns cache home
    And a run "ocsfegress" with an OCSF egress event
    When I run "lns audit ocsfegress"
    Then the exit code is 0
    And the output contains "egress"
    And the output contains "GET api.example.test:443"
    And the output contains "allowed once"
    And the output does not contain "class_uid"

  Scenario: the ledger reads OCSF connection and credential events
    Given a clean lns cache home
    And a connection ledger with OCSF events
    When I run "lns audit"
    Then the exit code is 0
    And the output contains "some-oauth"
    And the output contains "some-provider"

  Scenario: a single run log mixing legacy and OCSF lines reads both
    Given a clean lns cache home
    And a run "mixed" with a mixed legacy and OCSF audit log
    When I run "lns audit mixed"
    Then the exit code is 0
    And the output contains "GET api.example.test:443"
    And the output contains "data"

  Scenario: --json passes the OCSF ledger event through unchanged
    Given a clean lns cache home
    And a connection ledger with OCSF events
    When I run "lns audit --json"
    Then the exit code is 0
    And the output contains "class_uid"
    And the output contains "some-oauth"
