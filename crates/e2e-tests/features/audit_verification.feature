Feature: lns audit shows one unified timeline across all sandboxes
  `lns audit` merges every per-run audit log and the durable connection
  ledger into one chronological timeline; `lns audit <sandbox>` scopes it
  to a single run. Reading a tampered chain still lists events but warns
  about integrity.

  Scenario: a bare sandbox id scopes the timeline to that run
    Given a clean lns cache home
    And a run "smoke" with a valid audit chain
    When I run "lns audit smoke"
    Then the exit code is 0
    And the output contains "WHEN"

  Scenario: a guest egress event reads like the approval prompt
    Given a clean lns cache home
    And a run "egress" with a guest egress event
    When I run "lns audit egress"
    Then the exit code is 0
    And the output contains "egress"
    And the output contains "GET api.example.test:443"
    And the output contains "allowed once"
    And the output does not contain "class_uid"

  Scenario: an unknown sandbox reports no events without erroring
    Given a clean lns cache home
    When I run "lns audit nonexistent"
    Then the exit code is 0
    And the output contains "No audit events for sandbox nonexistent."

  Scenario: reading a tampered run chain still lists events but warns about integrity
    Given a clean lns cache home
    And a run "tamper" with a tampered audit chain
    When I run "lns audit tamper"
    Then the exit code is 0
    And the output contains "integrity"

  Scenario: stdout closing mid-write does not panic
    Given a clean lns cache home
    And a run "pipeable" with a valid audit chain
    When I run "lns audit pipeable" with stdout closed
    Then the exit code is non-zero
    And the output does not contain "panicked"

  Scenario: the unified timeline lists ledger connection events
    Given a clean lns cache home
    And a connection ledger with sample events
    When I run "lns audit"
    Then the exit code is 0
    And the output contains "some-oauth"
    And the output contains "some-provider"

  Scenario: reading a tampered ledger still lists events but warns about integrity
    Given a clean lns cache home
    And a connection ledger with a tampered event
    When I run "lns audit"
    Then the exit code is 0
    And the output contains "integrity"
    And the output contains "some-oauth"

  Scenario: a run's per-run log and its ledger events merge into one timeline, newest first
    Given a clean lns cache home
    And a run "41aaaaaa000000000000000000000000" with a guest egress event
    And a connection ledger with sample events
    When I run "lns audit 41aaaaaa000000000000000000000000"
    Then the exit code is 0
    And the output contains "some-oauth"
    And the output contains "GET api.example.test:443"
    And the output does not contain "some-provider"
    And the output shows "some-oauth" before "api.example.test"

  Scenario: --format jsonl emits raw events and suppresses the table
    Given a clean lns cache home
    And a connection ledger with sample events
    When I run "lns audit --format jsonl"
    Then the exit code is 0
    And the output contains "some-oauth"
    And the output contains "class_uid"
    And the output does not contain "WHEN"

  Scenario: --kind narrows the timeline to a single event kind
    Given a clean lns cache home
    And a run "41aaaaaa000000000000000000000000" with a guest egress event
    And a connection ledger with sample events
    When I run "lns audit 41aaaaaa000000000000000000000000 --kind connection"
    Then the exit code is 0
    And the output contains "some-oauth"
    And the output does not contain "api.example.test"
