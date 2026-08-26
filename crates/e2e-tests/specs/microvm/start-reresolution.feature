@microvm
Feature: lns start re-resolves policy live
  Parked until the e2e harness can stage a decisions-file edit between a
  stop and a start; the split itself (config frozen, policy live) is
  pinned at Layer 2.

  @microvm
  Scenario: network policy re-resolves at start
    Given a stopped run
    And a rule added to the decisions file after it was stopped
    When I start it
    Then the new rule applies to the restarted run
