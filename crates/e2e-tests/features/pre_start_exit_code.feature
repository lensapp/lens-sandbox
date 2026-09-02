Feature: a run or exec that fails before the workload answers 125

  §5: an error before the workload starts is `lns` failing, never the
  workload — so it must never be mistaken for a workload that exited 1.
  §1.3: `lns run` and `lns exec` name the same commands as their
  `lns sandbox` spellings, exit code included.

  Scenario: lns run answers 125 when the service is unreachable
    Given a clean lns cache home
    And no LNS service is running
    When I run "lns run some-image"
    Then the exit code is 125

  Scenario: lns sandbox run is the same command, 125 included
    Given a clean lns cache home
    And no LNS service is running
    When I run "lns sandbox run some-image"
    Then the exit code is 125

  Scenario: lns exec answers 125 when the service is unreachable
    Given a clean lns cache home
    And no LNS service is running
    When I run "lns exec 1 -- true"
    Then the exit code is 125

  Scenario: lns sandbox exec is the same command, 125 included
    Given a clean lns cache home
    And no LNS service is running
    When I run "lns sandbox exec 1 -- true"
    Then the exit code is 125
